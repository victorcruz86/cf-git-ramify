import {
  commitDetails,
  compareChangesBetween,
  compareChangesWith,
  compareLastRevision,
  compareLocalState,
  compareResolve,
  diffWindowOpenRequest,
  LOCAL_REVISION,
  repoState,
  type FileRevision,
  type LocalState,
  type CompareRefKind,
  type RevisionPair,
  type SideRef,
} from "../lib/api";
import {
  BASE_VERSION,
  branchChanges,
  showChanges,
  swapSidesSetting,
  YOUR_VERSION,
  type ChangesRequest,
} from "../changes/tab";
import { errorText } from "../lib/errors";
import type { LogChange } from "../log/changes";
import { emptyFilters, toLogFilter } from "../log/filters";
import { affectedTitle, historyKey, historyTabName } from "../log/history";
import { openLogTab } from "../log/tabKinds";

// `DiffRequestFactoryImpl.DIFF_TITLE_RENAME_SEPARATOR`.
const RENAME_SEPARATOR = " -> ";
// `DvcsUtil.getShortHash`, `Hash.toShortString`.
const SHORT_HASH = 8;
// `GitLogDiffHandler.getTitleForPaths`: `shortenTextWithEllipsis(joined, 100, 0)`.
const PATHS_TITLE_LIMIT = 100;
const PATHS_ELLIPSIS = "...";

// `V:104-105`, `B:1063-1064`.
const LOCAL_TITLE = "Local";
const localWithNumber = (hash: string) => `Local (${hash})`;
const LOCAL_VERSION_NAME = "local version";
// `VcsLogUtil.HEAD`: the Branch filter a history starts with.
const HEAD = "HEAD";
// `V:1010`.
export const CANNOT_GET_REVISION = "Cannot get revision content";

export const shortHash = (hash: string) => hash.slice(0, SHORT_HASH);

// A comparison that cannot be shown: the caller shows the text, as the diff window of 241 does.
export class CompareError extends Error {}

// A file or folder to compare. `path` is where it is now (or in the right revision); `revisionPath`
// is where it was in the left revision, `null` if it did not exist there.
export interface ComparePath {
  path: string;
  revisionPath?: string | null;
  folder?: boolean;
}

// A folder or several files go to the `Changes` tool window, one tab that replaces the open one.
export { showChanges, type ChangesRequest };

const nameOf = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `FilePath.getPresentableUrl` of the folder that holds `path`: the repository is its root.
function parentOf(root: string, path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? root : `${root}/${path.slice(0, slash)}`;
}

export const absolutePath = (root: string, path: string) => `${root}/${path}`;

// `DiffRequestFactoryImpl.getContentTitle`: `name (folder)`.
export function contentTitle(root: string, path: string): string {
  return `${nameOf(path)} (${parentOf(root, path)})`;
}

// `DiffRequestFactoryImpl.getTitle(path1, path2, " -> ")`, for files: one path, a rename in its
// folder, a move with the same name, or both.
export function requestTitle(root: string, left: string | null, right: string | null): string {
  if (left === null || right === null) return contentTitle(root, (left ?? right) as string);
  if (left === right) return contentTitle(root, left);
  const [leftName, rightName] = [nameOf(left), nameOf(right)];
  const [leftParent, rightParent] = [parentOf(root, left), parentOf(root, right)];
  if (leftParent === rightParent) {
    return `${leftName}${RENAME_SEPARATOR}${rightName} (${leftParent})`;
  }
  const names = leftName === rightName ? leftName : `${leftName}${RENAME_SEPARATOR}${rightName}`;
  return `${names} (${leftParent}${RENAME_SEPARATOR}${rightParent})`;
}

// `GitRevisionNumber.compareTo`: by commit time, and by hash when the times are equal.
function isNewer(revision: FileRevision, than: FileRevision): boolean {
  if (revision.hash === than.hash) return false;
  if (revision.commitTime !== than.commitTime) return revision.commitTime > than.commitTime;
  return revision.hash > than.hash;
}

const openPair = (pair: RevisionPair, title: string) =>
  diffWindowOpenRequest({ kind: "revisions", files: [pair], title });

// `DiffActionExecutor`: the revision against the local file. Without local changes the local side
// names the revision it has, and goes left if the chosen revision is newer.
function withLocal(
  state: LocalState,
  path: string,
  target: FileRevision,
  revisionPath: string,
): RevisionPair {
  const current = state.modified ? null : state.current;
  const remote: SideRef = { revision: target.hash, path: revisionPath, title: target.hash };
  const local: SideRef = {
    revision: LOCAL_REVISION,
    path,
    title: current ? localWithNumber(current.hash) : LOCAL_TITLE,
  };
  return current && isNewer(target, current)
    ? { left: local, right: remote }
    : { left: remote, right: local };
}

// `Compare with the Same Repository Version` (`A:273`).
export async function compareSameVersion(path: string): Promise<void> {
  const state = await compareLocalState(path);
  if (!state.current) throw new CompareError(CANNOT_GET_REVISION);
  await openPair(withLocal(state, path, state.current, path), contentTitle(state.root, path));
}

// `Compare with Revision...` (`A:275`) after the `File Revisions` popup: `revisionPath` is the path
// the file had in that revision.
export async function compareWithRevision(
  path: string,
  hash: string,
  revisionPath: string = path,
): Promise<void> {
  const [state, target] = await Promise.all([
    compareLocalState(path),
    compareLastRevision(hash, null),
  ]);
  if (!target) throw new CompareError(CANNOT_GET_REVISION);
  await openPair(withLocal(state, path, target, revisionPath), contentTitle(state.root, path));
}

// `D:102`, `B:1044`.
function notFoundText(kind: CompareRefKind, folder: boolean, file: string, name: string): string {
  const what = folder ? "Directory" : "File";
  return kind === "tag"
    ? `${what} ${file} doesn't exist in tag '${name}'`
    : `${what} ${file} doesn't exist in branch '${name}'`;
}

// `DvcsCompareWithAction.showDiffBetweenRevision`, after `getDiffChanges` of the branch or tag
// action: the reference against the working tree, `<ref>` / `<current> (Local)`.
async function compareWithRef(
  kind: CompareRefKind,
  path: string,
  name: string,
  folder: boolean,
): Promise<void> {
  const state = await compareLocalState(path);
  const missing = () =>
    new CompareError(notFoundText(kind, folder, absolutePath(state.root, path), name));
  const target = await compareResolve(kind, name);
  if (!target) throw missing();
  // A folder goes to `Changes`, whose Get needs the HEAD read before the list.
  const head = folder ? (await repoState()).head : null;
  const changes = await compareChangesWith(target.hash, [path]);
  if (changes.length === 0 && !(await compareLastRevision(target.hash, path))) throw missing();
  const leftTitle = name;
  const rightTitle = `${state.headLabel} (${LOCAL_TITLE})`;
  if (folder) {
    await showChanges({
      title: `Difference Between ${leftTitle} and ${rightTitle} Versions in ${nameOf(path)}`,
      left: target.hash,
      right: LOCAL_REVISION,
      files: changes,
      paths: [path],
      head,
    });
    return;
  }
  const change = changes[0];
  // `A`: the file is not in the reference; otherwise it was there with its old path.
  const leftPath = change?.status === "A" ? null : (change?.oldPath ?? path);
  const left: SideRef = {
    revision: leftPath === null ? null : target.hash,
    path: leftPath ?? path,
    title: leftTitle,
  };
  const right: SideRef = { revision: LOCAL_REVISION, path, title: rightTitle };
  await openPair({ left, right }, requestTitle(state.root, leftPath, path));
}

// `Compare with Branch…` (`B:585`), once a branch is chosen.
export const compareWithBranch = (path: string, branch: string, folder = false) =>
  compareWithRef("branch", path, branch, folder);

// `Compare with Tag…` (`B:586`): no menu entry in 241, only Find Action.
export const compareWithTag = (path: string, tag: string, folder = false) =>
  compareWithRef("tag", path, tag, folder);

const joinedPaths = (paths: ComparePath[]) => {
  const joined = paths.map((entry) => entry.path).join(", ");
  return joined.length > PATHS_TITLE_LIMIT
    ? joined.slice(0, PATHS_TITLE_LIMIT - PATHS_ELLIPSIS.length) + PATHS_ELLIPSIS
    : joined;
};

// `B:1061-1062`.
const changesTitle = (left: string, right: string, paths: ComparePath[]) =>
  paths.length > 0
    ? `Changes Between ${left} and ${right} in ${joinedPaths(paths)}`
    : `Changes Between ${left} and ${right}`;

const isOneFile = (paths: ComparePath[]) => paths.length === 1 && !paths[0].folder;

const leftPathOf = (entry: ComparePath) =>
  entry.revisionPath === undefined ? entry.path : entry.revisionPath;

// `Compare with Local` of a commit (`GitLogDiffHandler.showDiffWithLocal`, `showDiffForPaths`):
// one file in the diff window, `<hash>` / `Local`; folders or several files in `Changes`.
export async function compareWithLocal(hash: string, paths: ComparePath[]): Promise<void> {
  if (!isOneFile(paths)) {
    const read = paths.map((entry) => entry.path);
    const { head } = await repoState();
    const files = await compareChangesWith(hash, read);
    await showChanges({
      title: changesTitle(shortHash(hash), LOCAL_VERSION_NAME, paths),
      left: hash,
      right: LOCAL_REVISION,
      files,
      paths: read,
      head,
    });
    return;
  }
  const entry = paths[0];
  const state = await compareLocalState(entry.path);
  const leftPath = leftPathOf(entry);
  const left: SideRef = {
    revision: leftPath === null ? null : hash,
    path: leftPath ?? entry.path,
    title: hash,
  };
  const right: SideRef = { revision: LOCAL_REVISION, path: entry.path, title: LOCAL_TITLE };
  await openPair({ left, right }, requestTitle(state.root, leftPath, entry.path));
}

// `Compare Revisions` (`L:94`, `GitLogDiffHandler.showDiff`, `showDiffForPaths`): full hashes on
// both sides. `path` is the file in `right`, `revisionPath` in `left`.
export async function compareRevisions(
  left: string,
  right: string,
  paths: ComparePath[],
): Promise<void> {
  if (!isOneFile(paths)) {
    const read = paths.map((entry) => entry.path);
    const files = await compareChangesBetween(left, right, read);
    await showChanges({
      title: changesTitle(shortHash(left), shortHash(right), paths),
      left,
      right,
      files,
      paths: read,
    });
    return;
  }
  const entry = paths[0];
  const state = await compareLocalState(entry.path);
  const leftPath = leftPathOf(entry);
  await openPair(
    {
      left: {
        revision: leftPath === null ? null : left,
        path: leftPath ?? entry.path,
        title: left,
      },
      right: { revision: right, path: entry.path, title: right },
    },
    requestTitle(state.root, leftPath, entry.path),
  );
}

// `Show History` (`TabbedShowHistoryAction`): a file's history tab in the Log, reused by path and
// revision (`VcsLogFileHistoryProviderImpl`), with `HEAD` in its Branch filter; `revision` is that of
// `History Up to Here`, which has no Branch filter, and the tab jumps to it.
export async function showHistory(path: string, revision: string | null = null): Promise<void> {
  await openLogTab(
    "history",
    historyKey(path, revision),
    () => ({
      name: historyTabName(path, revision),
      filters: { ...emptyFilters(), branches: revision ? [] : [HEAD] },
      history: { path, revision },
    }),
    revision ? { select: revision } : {},
  );
}

// `VcsLogDirectoryHistoryProvider`: folders, or the repository itself (no path filter), go to a Log
// tab filtered by those paths and `HEAD`, with `Show Only Affected Changes` on; the tab with those
// filters is reused.
export async function showFolderHistory(paths: string[]): Promise<void> {
  const inside = paths.filter((path) => path !== "");
  const filters = { ...emptyFilters(), branches: [HEAD], paths: inside };
  const wanted = JSON.stringify(toLogFilter(filters));
  await openLogTab(
    "log",
    `paths\0${inside.join("\0")}`,
    () => ({ name: "", filters, options: { showOnlyAffected: true } }),
    { matches: (current) => JSON.stringify(toLogFilter(current)) === wanted },
  );
}

// What the Git file menu runs: one file, or folders only (`TabbedShowHistoryAction.isEnabled`).
export async function showHistoryOf(targets: { path: string; dir: boolean }[]): Promise<void> {
  if (targets.length === 1 && !targets[0].dir) await showHistory(targets[0].path);
  else if (targets.length > 0 && targets.every((target) => target.dir)) {
    await showFolderHistory(targets.map((target) => target.path));
  }
}

// `CompareRevisionsFromFileHistoryActionProvider`: the history's pair in the diff window, titled
// as `DiffRequestFactoryImpl.getTitle` names its paths.
export async function showHistoryDiff(pair: RevisionPair, path: string): Promise<void> {
  const state = await compareLocalState(path);
  const [left, right] = [
    pair.left.revision === null ? null : pair.left.path,
    (pair.center ?? pair.right).revision === null ? null : (pair.center ?? pair.right).path,
  ];
  await openPair(pair, requestTitle(state.root, left ?? right, right ?? left));
}

// `git hash-object -t tree /dev/null`: what the first commit is compared with.
const EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

// `ShowAllAffectedFromHistoryAction`: the commit's changes against its first parent in the
// `Changes` window, `Paths Affected by Commit {0}` (`L:117`), over its message, with the file of
// the history row chosen (`model.getPathInCommit`).
export async function showAllAffected(hash: string, path?: string): Promise<void> {
  const [details] = await commitDetails([hash]);
  const left = details?.parents[0] ?? EMPTY_TREE;
  const files = await compareChangesBetween(left, hash, []);
  const message = details ? [details.subject, details.body].filter(Boolean).join("\n\n") : undefined;
  await showChanges({ title: affectedTitle(hash), left, right: hash, files, message, select: path });
}

// `ShowDiffWithLocalAction` of a changes browser (`Compare with Local`, and with `before`
// `Compare Before with Local`): each change's after (or before) revision against the local file
// at that path, or at the change's other path when only that one is on disk (`getChangeWithLocal`).
// `onDisk` holds the paths of the changes that exist locally. All of them go to the diff window,
// which opens on `lead`, the change under the cursor (`WithLocalRequestChain` keeps the
// `ListSelection` index).
export async function compareChangesWithLocal(
  changes: LogChange[],
  before: boolean,
  onDisk: Set<string>,
  lead: LogChange | null = null,
): Promise<void> {
  const pairs: RevisionPair[] = [];
  let selected = 0;
  for (const change of changes) {
    const { file, hash, base } = change;
    const kind = file.status[0];
    const beforePath = file.oldPath ?? file.path;
    const revision = before
      ? kind === "A" || base === null
        ? null
        : { hash: base, path: beforePath }
      : kind === "D"
        ? null
        : { hash, path: file.path };
    const [own, other] = before ? [beforePath, file.path] : [file.path, beforePath];
    const local = onDisk.has(own) ? own : onDisk.has(other) ? other : null;
    if (revision === null && local === null) continue;
    if (change === lead) selected = pairs.length;
    pairs.push({
      left: revision
        ? { revision: revision.hash, path: revision.path, title: revision.hash }
        : { revision: null, path: local as string, title: BASE_VERSION },
      right: {
        revision: local === null ? null : LOCAL_REVISION,
        path: local ?? (revision as { path: string }).path,
        title: YOUR_VERSION,
      },
    });
  }
  if (pairs.length === 0) return;
  const first = pairs[selected];
  const state = await compareLocalState(first.right.path);
  const leftPath = first.left.revision === null ? null : first.left.path;
  const rightPath = first.right.revision === null ? null : first.right.path;
  await diffWindowOpenRequest({
    kind: "revisions",
    files: pairs,
    title: requestTitle(state.root, leftPath ?? rightPath, rightPath ?? leftPath),
    selected,
  });
}

// `GitCompareBranchesUi.getEditorTabName`: `StringUtil.shortenTextWithEllipsis(text, 150, 20)`.
const COMPARE_NAME_LIMIT = 150;
const COMPARE_NAME_SUFFIX = 20;

export function compareTabName(start: string, end: string): string {
  const text = `${end} and ${start}`;
  if (text.length <= COMPARE_NAME_LIMIT) return text;
  const prefix = COMPARE_NAME_LIMIT - COMPARE_NAME_SUFFIX - PATHS_ELLIPSIS.length;
  return text.slice(0, prefix) + PATHS_ELLIPSIS + text.slice(text.length - COMPARE_NAME_SUFFIX);
}

// `GitBrancher.compare` / `compareAny` (`GitCompareBranchesUi`): a Log tab with two logs,
// `other..branch` above and `branch..other` below; `other` is the current branch, or `HEAD` when
// it is detached. The tab of those ranges is reused (`GitCompareBranchesFilesManager.openFile`).
export async function compareBranches(branch: string, other: string): Promise<void> {
  await openLogTab("compare", `${other}..${branch}`, () => ({
    name: compareTabName(other, branch),
    filters: emptyFilters(),
    compare: { start: other, end: branch },
  }));
}

// `GitBrancher.showDiff` (`Show Files Diff`): the files between the heads of two branches,
// `git diff <one>..<two>`, in the `Changes` window, `Changes Between {0} and {1}` (`B:1061`).
export async function showBranchesDiff(one: string, two: string): Promise<void> {
  const [left, right] = await Promise.all([compareResolve("branch", one), compareResolve("branch", two)]);
  const missing = !left ? one : !right ? two : null;
  if (missing !== null || !left || !right) {
    throw new CompareError(`branch '${missing}' not found`);
  }
  const files = await compareChangesBetween(left.hash, right.hash, []);
  await showChanges({ title: `Changes Between ${one} and ${two}`, left: left.hash, right: right.hash, files });
}

// `B:1303`.
const couldNotLoadDiff = (branch: string, message: string) =>
  `Could not load diff with ${branch}: ${message}`;

// `GitShowDiffWithBranchPanel`: the branch's tree against the working tree in the `Changes` tool
// window, `Changes Between {0} and Current Working Tree` (`B:1301`). `git diff -R` puts the working
// tree on the left (`GitBranchWorker.loadTotalDiff`).
export async function showDiffWithWorkingTree(branch: string): Promise<void> {
  const read = async () => {
    // `GitBranchUtil.getCurrentBranchOrRev`; the HEAD the list goes with, which Get from Branch
    // must still find.
    const state = await repoState();
    const target = await compareResolve("branch", branch);
    if (!target) throw new Error(`branch '${branch}' not found`);
    const sides = await branchChanges(target.hash, swapSidesSetting());
    const current = state.branch ?? shortHash(state.head);
    return { ...sides, head: state.head, branch: { branch, current, hash: target.hash } };
  };
  const shown = await read().catch((error: unknown) => {
    throw new CompareError(couldNotLoadDiff(branch, errorText(error)));
  });
  await showChanges({ title: `Changes Between ${branch} and Current Working Tree`, ...shown });
}
