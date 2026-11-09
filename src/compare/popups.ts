import { mount, unmount, type Component } from "svelte";
import {
  branchList,
  compareFileRevisions,
  compareTags,
  type BranchList,
  type FileRevision,
} from "../lib/api";
import { errorText } from "../lib/errors";
import {
  compareWithBranch,
  compareWithLocal,
  compareWithRevision,
  compareWithTag,
  CompareError,
} from "./actions";
import FileRevisionsPopup from "./FileRevisionsPopup.svelte";
import ListPopup from "./ListPopup.svelte";

// `D:101`, `B:1043`, `B:1042`.
export const SELECT_BRANCH_TITLE = "Select Branch to Compare";
export const SELECT_TAG_TITLE = "Select Tag to Compare";
export const TAGS_ERROR_TITLE = "Cannot load tags";

// `StringUtil.compare(fullName, …, isFileSystemCaseSensitive)`: macOS volumes are not.
const byFullName = (prefix: string) => (left: string, right: string) => {
  const [a, b] = [`${prefix}${left}`.toLowerCase(), `${prefix}${right}`.toLowerCase()];
  return a < b ? -1 : a > b ? 1 : 0;
};

// `GitCompareWithBranchAction.getBranchNamesExceptCurrent`: the local branches but the current
// one, then the remote ones, each group sorted by its full name.
export function branchChoices(list: BranchList): string[] {
  const local = list.local
    .map((branch) => branch.name)
    .filter((name) => name !== list.current)
    .sort(byFullName("refs/heads/"));
  const remote = [...list.remote].sort(byFullName("refs/remotes/"));
  return [...local, ...remote];
}

// `GitCompareWithBranchAction.nothingToCompare`: the action is off without another branch.
export function nothingToCompare(list: BranchList): boolean {
  const haveRemotes = list.remote.length > 0;
  return list.current !== null
    ? list.local.length < 2 && !haveRemotes
    : list.local.length === 0 && !haveRemotes;
}

// Shows one popup in the middle of the window until it is closed; `null` if nothing was chosen.
function show<T, P extends Record<string, unknown>>(
  component: Component<P & { onchoose: (value: T) => void; onclose: () => void }>,
  props: P,
): Promise<T | null> {
  return new Promise((resolve) => {
    let chosen: T | null = null;
    let closed = false;
    const target = document.body;
    const instance = mount(component, {
      target,
      props: {
        ...props,
        onchoose: (value: T) => {
          chosen = value;
        },
        onclose: () => {
          if (closed) return;
          closed = true;
          // After the click or key that closes it has been handled.
          queueMicrotask(() => {
            unmount(instance);
            resolve(chosen);
          });
        },
      } as P & { onchoose: (value: T) => void; onclose: () => void },
    });
  });
}

const chooseFrom = (title: string, items: string[]) =>
  show<string, { title: string; items: string[] }>(ListPopup, { title, items });

// `Compare with Branch…`: `Select Branch to Compare`, then the comparison with the chosen branch.
export async function compareWithBranchPopup(path: string, folder = false): Promise<void> {
  const branch = await chooseFrom(SELECT_BRANCH_TITLE, branchChoices(await branchList()));
  if (branch !== null) await compareWithBranch(path, branch, folder);
}

// `Compare with Tag…`: `git tag -l` (`Loading Tags…`, `B:1041`), then `Select Tag to Compare`.
export async function compareWithTagPopup(path: string, folder = false): Promise<void> {
  let tags: string[];
  try {
    tags = await compareTags();
  } catch (failure) {
    throw new CompareError(`${TAGS_ERROR_TITLE}: ${errorText(failure)}`);
  }
  const tag = await chooseFrom(SELECT_TAG_TITLE, tags);
  if (tag !== null) await compareWithTag(path, tag, folder);
}

// `Compare with Revision...`: the `File Revisions` popup (`V:114`), then the chosen revision
// against the local file; a folder goes to the `Changes` window.
export async function compareWithRevisionPopup(path: string, folder = false): Promise<void> {
  const revisions = await compareFileRevisions(path);
  const chosen = await show<FileRevision, { revisions: FileRevision[]; now: number }>(
    FileRevisionsPopup,
    { revisions, now: Date.now() },
  );
  if (chosen === null) return;
  if (folder) await compareWithLocal(chosen.hash, [{ path, folder: true }]);
  else await compareWithRevision(path, chosen.hash, chosen.path ?? path);
}
