import { invoke, type Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { emitToOwner, listenOwn } from "./windows";

export interface DiffTiming {
  sidesMs: number;
  buildMs: number;
  builtAtEpochMs: number;
  // Null if no frame arrived: without a visible window there is no paint to measure.
  paintMs: number | null;
}

export interface ApiError {
  kind:
    | "git_not_found"
    | "not_a_repository"
    | "failed"
    | "unmerged"
    | "partial_merge"
    | "nothing_to_commit"
    | "gpg_failed"
    | "hook_failed"
    | "restore_failed"
    | "commit_running"
    | "selection_lost"
    // A file changed on disk after reading it (`conflict_resolve`): nothing was written.
    | "outdated"
    // `Get` (`get_version.rs`): the revision could not be read, or the file could not be written.
    | "load_failed"
    | "save_failed"
    // `github_login`: which field of the login dialog the error belongs to.
    | "login_server"
    | "login_token"
    // `github_connect`: the account has no token.
    | "missing_token"
    // GitHub refused the token (the list offers `Re-Login`).
    | "authentication"
    | "cancelled";
  message: string;
  // With "selection_lost": the file and the chosen hunks that no longer exist.
  details?: SelectionLost;
}

export interface SelectionLost {
  path: string;
  hunks: string[];
}

export interface RecoveryReport {
  restored: string[];
  skipped: string[];
  pending: boolean;
  error: string | null;
}

export interface RepoInfo {
  path: string;
  head: string | null;
  branch: string | null;
  recovery: RecoveryReport | null;
  pending: PendingRestore[];
  state: RepoState;
}

// What git has half done in the repository.
export type Operation = "merging" | "rebasing" | "cherryPicking" | "reverting";

export interface RepoState {
  operation: Operation | null;
  branch: string | null;
  // The text of the branch widget: `Rebasing main`, `Merging main`, the short hash…
  label: string;
  // The full HEAD, or empty without commits. The dialogs return it to check that nothing changed.
  head: string;
}

export interface PendingRestore {
  sha: string;
  title: string;
  message: string;
}

export interface LogSummary {
  viewId: number;
  count: number;
  // A details filter does not allow folding.
  collapsible: boolean;
}

export type SortType = "off" | "standard" | "linear";

export interface TextFilter {
  text: string;
  regex: boolean;
  matchCase: boolean;
}

// `branches` are patterns: names, regexes and exclusions with `-`. `revisions` are full hashes
// whose history is shown, and `hashes`, loose commits. `users` are names or `*`, the git user.
// `since` and `until` are in seconds since 1970.
export interface LogFilter {
  branches: string[];
  users: string[];
  since: number | null;
  until: number | null;
  paths: string[];
  text: TextFilter | null;
  hashes: string[];
  revisions: string[];
  ranges: [string, string][];
}

export interface ViewSpec {
  sort: SortType;
  longEdges: boolean;
  collapsed: boolean;
  filter: LogFilter;
}

// What the `logViewOpen` task returns.
export interface ViewOpened {
  viewId: number;
  summary: LogSummary;
}

export interface Person {
  name: string;
  email: string;
}

// Per filter, from newest to oldest.
export type RecentFilters = Record<string, string[][]>;

export interface ViewPatch {
  sort?: SortType;
  longEdges?: boolean;
  collapsed?: boolean;
}

export type EdgeStyle = "usual" | "dotted" | "dashed";

export interface ShortEdge {
  x: number;
  otherX: number;
  up: boolean;
  color: number;
  style: EdgeStyle;
  // The whole edge, in rows of the view.
  upRow: number;
  downRow: number;
}

export interface Arrow {
  x: number;
  otherX: number;
  up: boolean;
  color: number;
  targetRow: number;
  terminal: boolean;
  upRow: number;
  downRow: number;
}

// What the mouse points at in the graph.
export type GraphTarget =
  | { kind: "node"; row: number }
  | { kind: "edge"; up: number; down: number; style: EdgeStyle };

export interface PrintRow {
  nodeX: number;
  nodeColor: number;
  // In half columns.
  width: number;
  edges: ShortEdge[];
  arrows: Arrow[];
}

export type RefKind = "head" | "local" | "remote" | "tag";

export interface LogRef {
  name: string;
  kind: RefKind;
}

export interface Highlight {
  bold: boolean;
  muted: boolean;
  branch: boolean;
}

// A label of the table: its name and the kind of each reference it groups.
export interface RefLabel {
  name: string;
  kinds: RefKind[];
}

export interface LogRow {
  hash: string;
  subject: string;
  author: string;
  time: number;
  commitTime: number;
  // The committer is the author: if not, the column marks it with an asterisk.
  samePerson: boolean;
  // More than one: a merge, which Revert Commit does not support.
  parentCount: number;
  refs: LogRef[];
  // The two forms of `Compact References View`. Tags always carry their name.
  labels: { compact: RefLabel[]; normal: RefLabel[] };
  highlight: Highlight;
  print: PrintRow;
}

// Where `Go To Hash/Branch/Tag` goes.
export interface JumpTarget {
  kind: "ref" | "hash";
  value: string;
}

// Without `hash` the commit does not exist. With `hash` and without `row`, it does not pass the view's filters.
export interface RowOf {
  row: number | null;
  hash: string | null;
}

export interface GraphClicked {
  changed: boolean;
  summary: LogSummary;
}

export interface CommitDetails {
  hash: string;
  subject: string;
  body: string;
  author: string;
  authorEmail: string;
  authorTime: number;
  committer: string;
  committerEmail: string;
  commitTime: number;
  samePerson: boolean;
  refs: LogRef[];
  parents: string[];
  // What in the message names a commit of the log, with its full hash.
  links: Record<string, string>;
}

export type ChangeMode = "merged" | "parents";

// `base` is what it is compared against. `parent` and `subject` only appear in the "parents" mode of a merge.
export interface ChangeGroup {
  parent: string | null;
  subject: string | null;
  base: string | null;
  merge: boolean;
  files: ChangedFile[];
  // `VcsLogUtil.getSize`: the changes to all its parents.
  size: number;
}

// What the `commitChangesStart` task returns. `commits` arrives empty if `maxSize` goes over the
// requested limit.
export interface SelectedChanges {
  commits: { hash: string; group: ChangeGroup }[];
  maxSize: number;
}

export type Side =
  | { kind: "text"; content: string }
  | { kind: "binary" }
  | { kind: "tooLarge"; bytes: number }
  | { kind: "absent" };

export interface ChangedFile {
  path: string;
  oldPath: string | null;
  status: string;
}

export interface FileSides {
  old: Side;
  new: Side;
}

// The right side comes in the form git stores, like the hunks: its lines are valid for this text.
export interface WorktreeSides extends FileSides {
  hunks: Hunk[];
  // Only if saving the text leaves on disk the same bytes that were there.
  editable: boolean;
  // Why it cannot be edited, if it is not seen on its own side.
  note: string | null;
  // Of the bytes on disk, for `fileWrite`.
  hash: string | null;
  // With a conflict HEAD is shown against the disk, without actions on the stage.
  conflicted: boolean;
  // The path is a submodule: the full hash of the parent's HEAD, empty without a commit.
  submodule: string | null;
}

export type FileState =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "copied"
  | "unmerged"
  | "untracked"
  | "ignored";

export interface FileChange {
  path: string;
  oldPath: string | null;
  state: FileState;
  staged: boolean;
  unstaged: boolean;
}

export interface ChangeList {
  id: string;
  name: string;
  comment: string;
}

export interface Changelists {
  lists: ChangeList[];
  active: string;
  assignments: Record<string, string>;
  // Per file, the list of each hunk seen; a hunk without an entry follows the file's list.
  hunkAssignments: Record<string, Record<string, string>>;
}

export interface SnapshotEntry {
  path: string;
  kind: "file" | "symlink" | "absent";
  staged: boolean;
  stagedBlob: boolean;
}

export interface Snapshot {
  id: string;
  createdMs: number;
  reason: string;
  entries: SnapshotEntry[];
}

export interface PartialFile {
  path: string;
  content: number[];
}

// Contiguous change without context. Positions count lines from 0.
export interface Hunk {
  id: string;
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
}

// `lines: null` chooses the whole hunk. Otherwise it numbers the changed lines of the hunk in a single
// sequence: first the old ones (deletions), from 0 to oldLines - 1, and then the new ones
// (additions), from oldLines to oldLines + newLines - 1.
export interface LineSelection {
  hunk: string;
  lines: number[] | null;
}

export interface FileSelection {
  path: string;
  selection: LineSelection[];
}

export interface CommitOptions {
  amend: boolean;
  signOff: boolean;
  skipHooks: boolean;
  author: string | null;
  cleanup: boolean;
}

export type Comparison = "staged" | "local";
// What the diff window compares with the staging area: the two of `stagingSides`, or the three
// versions at once (`Compare HEAD, Staged and Local Versions`).
export type StageComparison = Comparison | "three";

// HEAD, the stage and the disk. `stagedHash` is the blob of the index entry and `localHash`, that of
// the bytes on disk: what is expected when writing each one.
export interface ThreeSides {
  head: string;
  staged: string;
  local: string;
  stagedHash: string;
  localHash: string;
}

// What `indexWrite` or `stagingRevertRange` wrote: the hash expected next time, and the copy
// saved before, if there was one.
export interface Written {
  hash: string;
  copy: string | null;
}

export interface LocalBranch {
  name: string;
  tracked: string | null;
}

export interface BranchList {
  local: LocalBranch[];
  remote: string[];
  recent: string[];
  current: string | null;
  previous: string | null;
}

export interface BranchSync {
  incoming: boolean;
  outgoing: boolean;
}

export type BranchKind = "local" | "remote";

export interface FavoriteSet {
  local: string[];
  remote: string[];
}

export type IncomingCheck = "auto" | "always" | "never";

export type UpdateMethod = "merge" | "rebase";

// `GeneralSettings.confirmOpenNewProject2`: where opening a project goes.
export type OpenProjectIn = "ask" | "new" | "current";

export interface Settings {
  gitPath: string | null;
  stagingArea: boolean;
  warnCrlf: boolean;
  warnDetached: boolean;
  // `Warn when committing files larger than [limit] MB`.
  warnLargeFiles: boolean;
  largeFilesLimitMb: number;
  warnBadFileNames: boolean;
  useCredentialHelper: boolean;
  incomingCheck: IncomingCheck;
  updateMethod: UpdateMethod;
  showUpdateDialog: boolean;
  // `Filter "Update Project" information by paths:`. Empty: all.
  updatePaths: string[];
  // `Auto-update if push of the current branch was rejected`.
  pushAutoUpdate: boolean;
  // `Show Push dialog for Commit and Push`.
  previewPushOnCommit: boolean;
  // `Show Push dialog only when committing to protected branches`.
  previewPushProtectedOnly: boolean;
  // `Automatically apply non-conflicting changes` of the merge window.
  mergeAutoApply: boolean;
  // The whitespace policy of the merge window.
  mergeWhitespace: Whitespace;
  // `Add the 'cherry-picked from <hash>' suffix when picking commits pushed to protected branches`.
  cherryPickSuffix: boolean;
  // `Use non-modal commit interface`. Off: the commit is a dialog.
  nonModalCommit: boolean;
  // `Clear initial commit message`.
  clearInitialCommitMessage: boolean;
  // `SHOW_DROP_COMMIT_DIALOG`.
  showDropCommitDialog: boolean;
  // `Connection timeout:` of the GitHub page, in milliseconds (`0..60000`, default 5000).
  githubConnectionTimeoutMs: number;
  // `Open project in:` (`confirmOpenNewProject2`).
  openProjectIn: OpenProjectIn;
  // `Reopen projects on startup` (`reopenLastProject`).
  reopenProjects: boolean;
}

export type CommitMode = "modal" | "non-modal";

export const commitMode = (settings: Settings): CommitMode =>
  settings.nonModalCommit ? "non-modal" : "modal";

// `Enable staging area` only counts in the non-modal interface (`canEnableStagingArea`): while modal it
// stays saved but is not in effect.
export const effectiveStaging = (settings: Settings): boolean =>
  settings.nonModalCommit && settings.stagingArea;

// `GitMergeOption`, in their order.
export type MergeOption =
  | "noFf"
  | "ffOnly"
  | "squash"
  | "message"
  | "noCommit"
  | "noVerify"
  | "allowUnrelatedHistories";

// The flags remembered by the Rebase… dialog, which are those of `REBASE_FLAGS`.
export type RebaseFlag = "rebaseMerges" | "keepEmpty" | "root" | "interactive" | "updateRefs";

// Per repository. `forcePushTargets`: the `Don't warn about this target` ones, as `remote:branch`.
export interface RepoSettings {
  protectedBranches: string[];
  forcePushTargets: string[];
  // `GitMergeSettings`: the branch and the options of the last Merge…, without the message.
  merge: { branch: string | null; options: MergeOption[] };
  // `GitRebaseSettings`: the flags and the base (the new one if there was `--onto`, otherwise the upstream).
  rebase: { options: RebaseFlag[]; newBase: string | null };
  cleanTree: CleanTree;
  shelf?: ShelfSettings;
  // The default GitHub account of this repository, as `<server>/<login>`.
  githubDefaultAccount?: string | null;
  // `Load branch protection rules from GitHub`.
  loadGithubProtection?: boolean;
  // The rules loaded from GitHub on the last fetches, already as regexes. Read-only: never saved.
  githubProtected?: string[];
}

// `remove_strategy`, `show_recycled` and the grouping of the Shelf tab, per repository.
export interface ShelfSettings {
  removeStrategy: boolean;
  showRecycled: boolean;
  groupByDirectory: boolean;
}

export type BadFileNameProblem = "reserved" | "invalidChar";

// `paths` of `largeFiles` are the files over the limit with their size in bytes; those of `badFileNames`, the
// first name of each that Windows does not accept, with the reason.
export type CommitWarning =
  | { kind: "crlf"; paths: string[] }
  | { kind: "largeFiles"; paths: [string, number][] }
  | { kind: "detachedHead" }
  | { kind: "rebasing" }
  | { kind: "badFileNames"; paths: { name: string; problem: BadFileNameProblem }[] };

export interface CommitRequest {
  added: string[];
  removed: string[];
  partial: PartialFile[];
  message: string;
  options: CommitOptions;
}

export const commitFiles = (hash: string) => invoke<ChangedFile[]>("commit_files", { hash });
// Without `base`, against the first parent. With `null`, the old side does not exist.
export const fileSides = (hash: string, file: ChangedFile, base?: string | null) =>
  invoke<FileSides>("file_sides", {
    hash,
    path: file.path,
    oldPath: file.oldPath,
    base: base ?? null,
    noBase: base === null,
  });
export const worktreeSides = (path: string) => invoke<WorktreeSides>("worktree_sides", { path });
export const worktreePreview = (path: string, content: string) =>
  invoke<Hunk[]>("worktree_preview", { path, content });
export const fileWrite = (path: string, content: string, expectedHash: string) =>
  invoke<string>("file_write", { path, content, expectedHash });
export const diffWindowOpen = (path: string) => invoke<void>("diff_window_open", { path });
export const diffWindowPath = () => invoke<string | null>("diff_window_path");
// Two revisions of one or more files (H§4), besides the working-tree request of `diffWindowOpen`.
// `revision` is a full hash, `"local"` for the file on disk, or `null` where the file is not.
export interface SideRef {
  revision: string | null;
  path: string;
  title: string;
}

export interface RevisionPair {
  left: SideRef;
  right: SideRef;
  // A merge of the file history: three read-only sides, the merge in the center.
  center?: SideRef;
}

export type DiffRequest =
  | { kind: "worktree"; path: string }
  // `selected`: the pair the window opens on, the first without it.
  | { kind: "revisions"; files: RevisionPair[]; title: string; selected?: number };

export const LOCAL_REVISION = "local";

export interface RevisionSides extends WorktreeSides {
  // The side is the first part of a file above the diff limit (`DB:273`).
  oldPreview: boolean;
  newPreview: boolean;
  // With a binary side: whether both have the same bytes (`DB:46-47`).
  identical: boolean | null;
}

export interface FileRevision {
  hash: string;
  // Seconds.
  commitTime: number;
  authorTime: number;
  author: string;
  message: string;
  // In the `File Revisions` popup, the path the file had in that revision (missing for a folder).
  path?: string;
}

export interface LocalState {
  root: string;
  modified: boolean;
  current: FileRevision | null;
  headLabel: string;
}

export type CompareRefKind = "branch" | "tag";

export const diffWindowOpenRequest = (request: DiffRequest) =>
  invoke<void>("diff_window_open_request", { request });
export const diffWindowRequest = () => invoke<DiffRequest | null>("diff_window_request");
export const compareSides = (left: SideRef, right: SideRef) =>
  invoke<RevisionSides>("compare_sides", { left, right });
export const comparePreview = (left: SideRef, right: SideRef, content: string) =>
  invoke<Hunk[]>("compare_preview", { left, right, content });
export const compareLocalState = (path: string) =>
  invoke<LocalState>("compare_local_state", { path });
// Without `revision`, the last commit of HEAD; without `path`, the commit itself.
export const compareLastRevision = (revision: string | null, path: string | null) =>
  invoke<FileRevision | null>("compare_last_revision", { revision, path });
export const compareResolve = (kind: CompareRefKind, name: string) =>
  invoke<FileRevision | null>("compare_resolve", { kind, name });
export const compareFileRevisions = (path: string) =>
  invoke<FileRevision[]>("compare_file_revisions", { path });
export const compareChangesWith = (revision: string, paths: string[]) =>
  invoke<ChangedFile[]>("compare_changes_with", { revision, paths });
export const compareChangesBetween = (left: string, right: string, paths: string[]) =>
  invoke<ChangedFile[]>("compare_changes_between", { left, right, paths });
// `Show Diff with Working Tree`: the working tree on the left, the revision on the right.
export const compareChangesFromWorkingTree = (revision: string) =>
  invoke<ChangedFile[]>("compare_changes_from_working_tree", { revision });
export const compareTags = () => invoke<string[]>("compare_tags");
// `Show Current Revision`: the last commit of any branch that touched the file; `author` has the
// committer after it when that is someone else.
export interface CurrentRevision {
  hash: string;
  commitTime: number;
  author: string;
  message: string;
}
export const compareCurrentRevision = (path: string) =>
  invoke<CurrentRevision | null>("compare_current_revision", { path });
// `Get` (`src-tauri/src/get_version.rs`): `path` is where the file is now, `oldPath` the path it had
// in `revision` when that is another one; the file is written at the path of the revision.
export interface GetItem {
  path: string;
  revision: string;
  oldPath?: string | null;
}
// The local files that exist and have changes (the `Get Revision` question), and the HEAD the
// write must still find.
export interface GetCheck {
  root: string;
  head: string;
  modified: string[];
  seen: GetSeen[];
}
// What the check read of each path the Get touches (`null`: nothing there). The write leaves a
// file that no longer holds it.
export interface GetSeen {
  path: string;
  hash: string | null;
}
// The recoverable copy taken before writing, the files written, and the files left because they
// changed after the check. A write that fails after the copy carries the same in `details`.
export interface GotRevision {
  copy: string;
  written: string[];
  changed: string[];
}
// It saves the diff window first.
export const revisionGetCheck = (items: GetItem[]) =>
  invoke<GetCheck>("revision_get_check", { items });
export const revisionGet = (items: GetItem[], expectedHead: string, seen: GetSeen[]) =>
  invoke<GotRevision>("revision_get", { items, expectedHead, seen });
export const diffFlushed = (saved: boolean) => invoke<void>("diff_flushed", { saved });
// Decision 3: with the merge window open, it brings it to the front with the file it already had.
export const mergeWindowOpen = (path: string) => invoke<void>("merge_window_open", { path });
export const mergeWindowPath = () => invoke<string | null>("merge_window_path");
// The contents mode: three texts with no file in conflict behind them. With the window already open it
// is refused. Its `merge-done` arrives through `onContentsMergeDone`, with this `id`.
export const mergeWindowOpenContents = (input: MergeContents) =>
  invoke<void>("merge_window_open_contents", { input });
export const mergeWindowContents = () => invoke<MergeContents | null>("merge_window_contents");
// Finishes the merge window: the main one receives `merge-done` and the window is destroyed. `content`
// is the result of the contents mode; a conflict is saved by the window itself.
export const mergeWindowDone = (result: MergeResult, content?: string) =>
  invoke<void>("merge_window_done", content === undefined ? { result } : { result, content });
export const mergeWorkingFile = (path: string) =>
  invoke<WorkingFile>("merge_working_file", { path });
// `merge-leave` is answered in two steps: the acknowledgement right away, and then `closed` if the merge can be
// thrown away, once the user has answered.
export const mergeLeaveReceived = () => invoke<void>("merge_leave_received");
export const mergeLeft = (closed: boolean) => invoke<void>("merge_left", { closed });
// The page already listens for its notices.
export const mergeWindowReady = () => invoke<void>("merge_window_ready");
export const repoStatus = (includeIgnored: boolean) =>
  invoke<FileChange[]>("repo_status", { includeIgnored });
export const changelistsGet = (paths: string[]) =>
  invoke<Changelists>("changelists_get", { paths });
export const changelistsCreate = (name: string) =>
  invoke<Changelists>("changelists_create", { name });
export const changelistsRename = (id: string, name: string) =>
  invoke<Changelists>("changelists_rename", { id, name });
// `repo`: the repository the comment was written for; if another one is open now, nothing is saved.
export const changelistsSetComment = (id: string, comment: string, repo: string) =>
  invoke<Changelists>("changelists_set_comment", { id, comment, repo });
export const changelistsRemove = (id: string) => invoke<Changelists>("changelists_remove", { id });
export const changelistsSetActive = (id: string) =>
  invoke<Changelists>("changelists_set_active", { id });
export const changelistsMove = (paths: string[], id: string) =>
  invoke<Changelists>("changelists_move", { paths, id });
export const changelistsImport = () => invoke<Changelists>("changelists_import");
export const rollbackFiles = (paths: string[], deleteAdded: boolean) =>
  invoke<string>("rollback_files", { paths, deleteAdded });
export const commitChanges = (request: CommitRequest) =>
  invoke<string>("commit_changes", { request });
export const fileHunks = (path: string) => invoke<Hunk[]>("file_hunks", { path });
export const linesRollback = (path: string, selection: LineSelection[]) =>
  invoke<string>("lines_rollback", { path, selection });
export const changelistsMoveHunks = (path: string, hunks: string[], id: string) =>
  invoke<Changelists>("changelists_move_hunks", { path, hunks, id });
// The content of partial files is built by the backend from the selection: the bytes do not
// go through JSON.
export const commitChangesSelected = (request: CommitRequest, selections: FileSelection[]) =>
  invoke<string>("commit_changes_selected", { request, selections });
export const commitWarnings = (request: CommitRequest, selections: FileSelection[]) =>
  invoke<CommitWarning[]>("commit_warnings", { request, selections });
export const commitWarningsStaged = () => invoke<CommitWarning[]>("commit_warnings_staged");
export const commitFixAutocrlf = () => invoke<void>("commit_fix_autocrlf");
export const commitStaged = (options: CommitOptions, message: string) =>
  invoke<string>("commit_staged", { options, message });
// They return the saved copy, if there was something that was only in the index.
export const stagingStage = (paths: string[]) =>
  invoke<string | null>("staging_stage", { paths });
export const stagingUnstage = (paths: string[]) =>
  invoke<string | null>("staging_unstage", { paths });
// The disk goes back to what is in the index. It always saves a copy, except of what is not a file.
export const stagingRevert = (paths: string[]) =>
  invoke<string | null>("staging_revert", { paths });
// The hunks of `stagingSides`: those of "local" are taken to the index and those of "staged" are taken out.
export const stagingStageHunks = (path: string, selection: LineSelection[]) =>
  invoke<string | null>("staging_stage_hunks", { path, selection });
export const stagingUnstageHunks = (path: string, selection: LineSelection[]) =>
  invoke<string | null>("staging_unstage_hunks", { path, selection });
// `Add to VCS` with changelists, for the repository `repo`: launches a task whose result is the saved
// copy, if there was something that was only in the index.
export const changesAdd = (repo: string, paths: string[]) =>
  invoke<number>("changes_add", { repo, paths });
export const stagingSides = (path: string, comparison: Comparison) =>
  invoke<WorktreeSides>("staging_sides", { path, comparison });
export const stagingPreview = (path: string, content: string) =>
  invoke<Hunk[]>("staging_preview", { path, content });
// Fails with `no_three_sides` if any of the three versions is missing or is not editable text.
export const stagingThreeSides = (path: string) =>
  invoke<ThreeSides>("staging_three_sides", { path });
export const indexWrite = (path: string, content: string, expectedHash: string) =>
  invoke<Written>("index_write", { path, content, expectedHash });
// Like `fileWrite`, after saving a copy of the file.
export const stagingRevertRange = (path: string, content: string, expectedHash: string) =>
  invoke<Written>("staging_revert_range", { path, content, expectedHash });
export const settingsGet = () => invoke<Settings>("settings_get");
export const settingsSave = (settings: Settings) =>
  invoke<Settings>("settings_save", { settings });
// The version of a path not yet saved; without a path, that of the detected one.
export const gitVersion = (path: string | null) => invoke<string>("git_version", { path });
export const messagesList = () => invoke<string[]>("messages_list");
export const headMessage = () => invoke<string>("head_message");
// `LAST_COMMIT_MESSAGE`: the message of the last commit made through the app, if any.
export const lastCommitMessage = () => invoke<string | null>("messages_last");
// Into the history, as the text that was there before amend loaded the last commit's message.
export const messagesPush = (message: string) => invoke<void>("messages_push", { message });
// An executable `pre-commit` or `commit-msg`: only then is `Run Git hooks` offered.
export const repoHasCommitHooks = () => invoke<boolean>("repo_has_commit_hooks");
export const snapshotsList = () => invoke<Snapshot[]>("snapshots_list");
// `expectedRepo`: the repository the copy was taken in; another one open says whose it is.
export const snapshotsRestore = (id: string, expectedRepo?: string) =>
  invoke<string>("snapshots_restore", expectedRepo ? { id, expectedRepo } : { id });
export const perfEnabled = () => invoke<boolean>("perf_enabled");
export const perfReport = (json: string) => invoke<void>("perf_report", { json });
export const appVersion = () => invoke<string>("app_version");
export const initialRepo = () => invoke<string | null>("initial_repo");
// `Ramify <file>[:<line>]`: the file's repository, and the file relative to it. A folder comes as
// `repo` alone, as `initialRepo` gives it.
export interface InitialTarget {
  repo: string;
  file: string | null;
  line: number | null;
}
export const initialTarget = () => invoke<InitialTarget | null>("initial_target");
// A file of the viewer: on disk without `revision` (a full hash), and the line to show, from 1.
export interface ViewerTarget {
  path: string;
  revision: string | null;
  line: number | null;
  // `Annotate` from a menu outside the viewer: the tab shows its annotations once open.
  annotate?: boolean;
}
// Rust owns the limits: a preview says its whole size and how much it shows; a binary says whether
// it is over the content limit.
export type ViewerContent =
  | { kind: "text"; text: string; previewOf: number | null; previewSize: number | null }
  | { kind: "binary"; size: number; tooLarge: boolean }
  | { kind: "absent" };
// Local files that are not on disk are skipped; with nothing left, the window does not open.
export const viewerOpen = (targets: ViewerTarget[]) => invoke<void>("viewer_open", { targets });
// The targets `viewerOpen` would open: the others are local files not on disk.
export const viewerOpenable = (targets: ViewerTarget[]) =>
  invoke<ViewerTarget[]>("viewer_openable", { targets });
// Size, inode and modification time of a local file, without reading it; null when it is not one.
export const viewerStamp = (path: string) => invoke<string | null>("viewer_stamp", { path });
// What `viewerOpen` left for the viewer page, taken once.
export const viewerTargets = () => invoke<ViewerTarget[]>("viewer_targets");
export const viewerContent = (path: string, revision: string | null) =>
  invoke<ViewerContent>("viewer_content", { path, revision });
// `viewer_open` sends it to the viewer only (`emit_to`).
export const onViewerOpen = (handler: () => void) =>
  listenOwn("viewer-open", () => handler());
// A file in a commit (`GitFileRevision`): what `Annotate Revision` and `Annotate Previous
// Revision` open.
export interface AnnotationTarget {
  hash: string;
  path: string;
}
// A commit of an annotation (`GitFileAnnotation.CommitInfo`); times in seconds. `previous` is what
// `Annotate Previous Revision` opens, `number` its place in the file history (the oldest is 1),
// and the colours, the background in the `Order` and `Author` modes.
export interface AnnotatedCommit {
  hash: string;
  path: string;
  author: string;
  email: string;
  authorTime: number;
  committerTime: number;
  subject: string;
  message: string;
  previous: AnnotationTarget | null;
  number: number | null;
  orderColor: string | null;
  authorColor: string | null;
}
// `revision` is the one blamed (the tab's, or the last commit of the local file), `path` the file
// in it. `lines[i]` is the commit of line `i + 1` of the text shown, `null` for a line changed
// locally, and `origins[i]` its line in the revision blamed (from 0; a changed line, where its
// change starts). `owners` is the commit of each line blamed. `suspicious`: the line counts differ
// as `V:687` warns. `historySize` is 0 without numbers; `line`, the line of `from` in this revision.
export interface Annotation {
  revision: string;
  path: string;
  commits: AnnotatedCommit[];
  lines: (number | null)[];
  origins: number[];
  owners: number[];
  suspicious: boolean;
  historySize: number;
  line: number | null;
}
export interface AnnotateRequest {
  path: string;
  revision: string | null;
  // The local file's text as the viewer shows it.
  localText: string | null;
  // The line (from 1) of the text an annotation is opened from.
  from: { text: string; line: number } | null;
}
// Launches `Loading Annotations`; the result, an `Annotation`, arrives with `taskResult`.
export const annotateStart = (request: AnnotateRequest) =>
  invoke<number>("annotate_start", { request });
// A local file's annotation after a change on disk or in the repository: closed when its base
// revision moved, else its lines mapped again from `owners`.
export interface AnnotateRefreshRequest {
  path: string;
  revision: string;
  blamedPath: string;
  owners: number[];
  localText: string;
}
export type AnnotationRefreshed =
  | { kind: "closed" }
  | { kind: "lines"; lines: (number | null)[]; origins: number[] };
export const annotateRefresh = (request: AnnotateRefreshRequest) =>
  invoke<AnnotationRefreshed>("annotate_refresh", { request });
// A click on an annotated line: the main window selects the commit in its Log.
export const annotationShowInLog = (hash: string) =>
  invoke<void>("annotation_show_in_log", { hash });
export const onAnnotationShowInLog = (handler: (hash: string) => void) =>
  listenOwn<string>("annotation-show-in-log", (event) => handler(event.payload));
// Show History for Selection: the local file, its selected lines (from 0, both included) and its
// text as the viewer shows it. `stream` tells one run's updates from another's.
export interface SelectionRequest {
  path: string;
  startLine: number;
  endLine: number;
  localText: string;
  stream: number;
}
// A revision of the file's history (`GitFileRevision`); `path` is the file's in it, the time in
// seconds.
export interface SelectionRevision {
  hash: string;
  path: string;
  commitTime: number;
  author: string;
  authorEmail: string;
  committer: string;
  committerEmail: string;
  message: string;
}
// The lines of a block and the line (from 0) of its text where they start.
export interface BlockLines {
  start: number;
  lines: string[];
}
// The block of the local text comes first (index 0), then the revisions, then one block per
// revision (index `i` is revision `i - 1`); a `null` block is gone from that revision on.
export type SelectionUpdate =
  | { kind: "revisions"; stream: number; localTime: number | null; revisions: SelectionRevision[] }
  | { kind: "block"; stream: number; index: number; block: BlockLines | null };
// Opens (or reuses) the `History for Selection` window with this selection.
export const selectionHistoryOpen = (request: SelectionRequest) =>
  invoke<void>("selection_history_open", { request });
// What `selectionHistoryOpen` left for the window's page, taken once.
export const selectionHistoryTarget = () =>
  invoke<SelectionRequest | null>("selection_history_target");
// The stream's task: its updates arrive with `onSelectionHistoryUpdate`, its end with `taskResult`.
export const selectionHistoryStart = (request: SelectionRequest) =>
  invoke<number>("selection_history_start", { request });
// Cancels the stream and closes the window.
export const selectionHistoryClose = () => invoke<void>("selection_history_close");
export const onSelectionHistoryOpen = (handler: () => void) =>
  listenOwn("selection-history-open", () => handler());
export const onSelectionHistoryUpdate = (handler: (update: SelectionUpdate) => void) =>
  listenOwn<SelectionUpdate>("selection-history-update", (event) => handler(event.payload));
// `Show All Affected Files` of a row, which the main window's `Changes` window shows.
export interface AffectedFiles {
  hash: string;
  path: string;
}
export const selectionHistoryShowAffected = (affected: AffectedFiles) =>
  invoke<void>("selection_history_show_affected", { affected });
export const onSelectionHistoryShowAffected = (handler: (affected: AffectedFiles) => void) =>
  listenOwn<AffectedFiles>("selection-history-show-affected", (event) => handler(event.payload));
// An entry of the Project tree: `path` is relative to the repository, exactly as the disk spells it.
export interface TreeEntry {
  name: string;
  path: string;
  dir: boolean;
  // A file of a type IntelliJ registers as binary: Rust says it, by the name (always sent; the
  // repository root row the tree makes has none).
  binary?: boolean;
}
// The entries of a folder of the repository ("" for its root) as the disk has them now: folders
// first, by natural name, without IntelliJ's default ignore patterns.
export const treeList = (dir: string) => invoke<TreeEntry[]>("tree_list", { dir });
// The entries of a folder ("" for the root) at `revision`, a full hash, as `git ls-tree` gives them:
// folders first, by natural name, without what IntelliJ ignores and without archives.
export const treeListAt = (revision: string, dir: string) =>
  invoke<TreeEntry[]>("tree_list_at", { revision, dir });
export const showWindow = () => invoke<void>("show_window");
export const hideWindow = () => invoke<void>("hide_window");
export const repoOpen = (repo: string) => invoke<RepoInfo>("repo_open", { repo });
// The open flow (`src/project/open.ts`). The repository root of a folder, as `repoOpen` would
// open it: a subfolder gives its root, a folder outside any repository fails.
export const projectResolve = (target: string) => invoke<string>("project_resolve", { target });
// `true` if another window has `root` open: it was brought to the front.
export const projectFocusOpen = (root: string) => invoke<boolean>("project_focus_open", { root });
// The project widget's popup: the repositories open in the other windows, and the recent ones that
// are not open in any window (at most 100). `homePath` is the path from `~` when it is inside the
// home folder.
export interface ProjectRow {
  path: string;
  name: string;
  homePath: string;
  // Whether the folder is still there.
  exists: boolean;
}
export interface ProjectsList {
  open: ProjectRow[];
  recent: ProjectRow[];
}
export const projectsList = () => invoke<ProjectsList>("projects_list");
// A new main window that opens `root`. `created: false`: another window already had `root` open
// or was opening it, and was brought to the front instead.
export interface NewWindow {
  label: string;
  created: boolean;
}
export const projectNewWindow = (root: string) => invoke<NewWindow>("project_new_window", { root });
// Reopening on start: the first `main` page, once it has its initial target (the open list's first
// project), asks for the others, which open in new windows. `missing` are the paths of the list
// that no longer exist, which this window reports.
export interface Reopened {
  missing: string[];
}
export const openProjectsReopen = () => invoke<Reopened>("open_projects_reopen");
// The answer to `commit-draft-save`: the draft is saved, the window may close.
export const commitDraftSaved = () => invoke<void>("commit_draft_saved");
export const repoState = () => invoke<RepoState>("repo_state");
// Turns on in the Git menu the entries of the operation in progress that apply, and turns off the others.
export const gitMenuEnable = (ids: string[]) => invoke<void>("git_menu_enable", { ids });
// An entry of `Git > Current File` as the selection of the focused window has it.
export interface CurrentFileItem {
  id: string;
  text: string;
  enabled: boolean;
}
// `GitFileActionGroup`: the title and the entries of `Git > Current File`; an entry not given is off.
export const gitMenuCurrentFile = (title: string, items: CurrentFileItem[]) =>
  invoke<void>("git_menu_current_file", { title, items });
// A file action of the viewer's editor that the main window carries out, with its files.
export interface GitFileActionRequest {
  id: string;
  paths: string[];
}
export const gitFileAction = (action: GitFileActionRequest) =>
  invoke<void>("git_file_action", { action });
export const onGitFileAction = (handler: (action: GitFileActionRequest) => void) =>
  listenOwn<GitFileActionRequest>("git-file-action", (event) => handler(event.payload));
export const logOpen = () => invoke<LogSummary>("log_open");
export const logRows = (viewId: number, start: number, count: number) =>
  invoke<LogRow[]>("log_rows", { viewId, start, count });
export const logViewSet = (viewId: number, patch: ViewPatch) =>
  invoke<LogSummary>("log_view_set", { viewId, patch });
export const logGraphClick = (viewId: number, target: GraphTarget) =>
  invoke<GraphClicked>("log_graph_click", { viewId, target });
// The rows that get highlighted on hover, or `null` if there is nothing to highlight.
export const logGraphHover = (viewId: number, target: GraphTarget) =>
  invoke<number[] | null>("log_graph_hover", { viewId, target });
export const logRowOf = (viewId: number, target: JumpTarget) =>
  invoke<RowOf>("log_row_of", { viewId, target });
export const logAdjacentRows = (viewId: number, row: number, parent: boolean) =>
  invoke<number[]>("log_adjacent_rows", { viewId, row, parent });
export const logCurrentBranch = () => invoke<string | null>("log_current_branch");
export const clipboardWrite = (text: string) => invoke<void>("clipboard_write", { text });
// Returns the task id: its result is a `ViewOpened`, and meanwhile the previous view
// stays in place. Another call from the same tab cancels this one.
export const logViewOpen = (tab: number, spec: ViewSpec) =>
  invoke<number>("log_view_open", { tab, spec });
// A file history tab (`src-tauri/src/history.rs`): the task's result is a `ViewOpened` too.
// `revision` is that of `History Up to Here`, which ignores `branches`; empty `branches` is every
// reference.
export interface HistoryRequest {
  path: string;
  revision: string | null;
  branches: string[];
}
// The file in a commit of the history.
export interface HistoryEntry {
  path: string;
  deleted: boolean;
}
export interface HistorySide {
  hash: string;
  path: string;
}
// A row against its parent rows: no `after` where the row deleted the file, `null` for a parent
// without it, two `before` for a merge, none for the first commit.
export interface HistoryChange {
  hash: string;
  path: string;
  after: HistorySide | null;
  before: (HistorySide | null)[];
}
export const logHistoryOpen = (tab: number, spec: ViewSpec, request: HistoryRequest) =>
  invoke<number>("log_history_open", { tab, spec, request });
export const logHistoryEntries = (viewId: number, hashes: string[]) =>
  invoke<(HistoryEntry | null)[]>("log_history_entries", { viewId, hashes });
export const logHistoryChange = (viewId: number, hashes: string[]) =>
  invoke<HistoryChange | null>("log_history_change", { viewId, hashes });
export const logUsers = () => invoke<Person[]>("log_users");
export const logRefs = () => invoke<LogRef[]>("log_refs");
export const branchList = () => invoke<BranchList>("branch_list");
export const branchIncomingOutgoing = () =>
  invoke<Record<string, BranchSync>>("branch_incoming_outgoing");
export const branchRefsFingerprint = () => invoke<string>("branch_refs_fingerprint");
export const favoritesGet = () => invoke<FavoriteSet>("favorites_get");
export const favoritesToggle = (name: string, kind: BranchKind) =>
  invoke<FavoriteSet>("favorites_toggle", { name, kind });
// What is remembered of each repository: the options of the last Pull and the mode of the last Reset.
export const pullOptionsGet = () => invoke<PullOption[]>("pull_options_get");
export const pullOptionsSet = (options: PullOption[]) =>
  invoke<void>("pull_options_set", { options });
export const resetModeGet = () => invoke<ResetMode | null>("reset_mode_get");
export const resetModeSet = (mode: ResetMode) => invoke<void>("reset_mode_set", { mode });
export const branchOptionsGet = () => invoke<Record<string, unknown>>("branch_options_get");
export const branchOptionsSet = (options: unknown) =>
  invoke<void>("branch_options_set", { options });
export const onBranchSyncChanged = (handler: () => void) =>
  listenOwn("branch-sync-changed", () => handler());
export const logViewClose = (tab: number) => invoke<void>("log_view_close", { tab });
// By hash, its row in the view. Null if it is not seen.
export const logFindRows = (viewId: number, hashes: string[]) =>
  invoke<(number | null)[]>("log_find_rows", { viewId, hashes });
export const logRefsChanged = () => invoke<boolean>("log_refs_changed");
export const logOptionsGet = () => invoke<Record<string, unknown>>("log_options_get");
export const logOptionsSet = (options: Record<string, unknown>) =>
  invoke<void>("log_options_set", { options });
export const commitDetails = (hashes: string[]) =>
  invoke<CommitDetails[]>("commit_details", { hashes });
export const branchesContaining = (hash: string) =>
  invoke<string[]>("branches_containing", { hash });
export const commitChangesOf = (hash: string, mode: ChangeMode) =>
  invoke<ChangeGroup[]>("commit_changes_of", { hash, mode });
// Those of several commits, in a single git command. Returns the task id, which is cancelled if
// the selection changes: its result is a `SelectedChanges`.
export const commitChangesStart = (hashes: string[], limit: number | null) =>
  invoke<number>("commit_changes_start", { hashes, limit });
// `GitCommitSignature`: `N` (unsigned) has no entry.
export type SignatureStatus =
  | "verified"
  | "unknown"
  | "expired"
  | "expiredKey"
  | "revokedKey"
  | "cannotVerify"
  | "bad";

export interface Signature {
  hash: string;
  status: SignatureStatus;
  signer: string;
  fingerprint: string;
}

// The signatures of some commits (`git log --no-walk`), for the `GPG Signature` column and the
// details. Returns the id of a task whose result is a `Signature[]`. `requester` is the table or
// panel that asks: its new request cancels its previous one.
export const logSignatures = (hashes: string[], requester: number) =>
  invoke<number>("log_signatures", { hashes, requester });
export const logRecentFilters = () => invoke<RecentFilters>("log_recent_filters");
export const logRememberFilter = (filter: string, values: string[]) =>
  invoke<RecentFilters>("log_remember_filter", { filter, values });
// `Not Cherry-Picked Commits`: the branch a tab's filters compare with, if any.
export const logComparedBranch = (filter: LogFilter) =>
  invoke<string | null>("log_compared_branch", { filter });
// The id of a task whose result is a `NotPicked`, or null when `branch` is not a branch of the log.
export const logNotPicked = (branch: string) => invoke<number | null>("log_not_picked", { branch });

export interface NotPicked {
  // The current branch compared with, `HEAD` when detached.
  current: string;
  // The commits of the compared branch with no equivalent patch in the current one.
  commits: string[];
}
export type CheckoutMode = "plain" | "smart" | "force";
export type ResetMode = "soft" | "mixed" | "hard" | "keep";
export type Overwrite = "no" | "try" | "drop";

// `Clean working tree using:` (`GitSaveChangesPolicy`), per repository.
export type CleanTree = "stash" | "shelve";

// A system list of the shelf: its name in the store, what the user sees (`<message> [<list>]`) and
// the changelist it came from (`null` with the staging area).
export interface SavedList {
  name: string;
  description: string;
  changelist: string | null;
}

// What a saver kept: the stash entry by `sha`, or the shelved `lists`. A record of an earlier build
// is a stash entry.
export interface SavedChanges {
  kind: CleanTree;
  sha: string;
  message: string;
  lists: SavedList[];
}

export type CheckoutOutcome =
  | { kind: "done" }
  | { kind: "overwritten"; files: string[] }
  // `failure`: the operation also failed, but what was saved is already in the tree.
  | { kind: "conflicts"; stash: SavedChanges; failure: string | null }
  | { kind: "restoredWithoutIndex"; stash: SavedChanges; failure: string | null }
  | { kind: "notRestored"; stash: SavedChanges; reason: string }
  | { kind: "gone"; stash: SavedChanges; failure: string | null };

export type RemoteCheckout =
  | CheckoutOutcome
  | { kind: "needsName"; suggested: string }
  | { kind: "localHasCommits"; local: string };

export type RebaseOutcome =
  | { kind: "done" }
  | { kind: "conflicts" }
  | { kind: "editing" }
  | { kind: "untracked"; reason: { files: string[] } }
  | { kind: "failed"; reason: string };

// What was saved before a rebase. `restore` is `null` if returning it was not attempted: the rebase was left
// half done and what was saved stays in the stash until it ends.
export interface KeptChanges {
  saved: SavedChanges;
  restore: RestoreOutcome | null;
}

// `rebase::Process`. With `title`, the rebase did not even start (`Rebase not allowed`, `Rebase not
// started`): it is the one of the notification, and `outcome` is `failed` with the text.
export interface RebaseRun {
  outcome: RebaseOutcome;
  title: string | null;
  saved: KeptChanges[];
}

export type RemoteRebase =
  | { kind: "rebased"; reason: RebaseRun }
  | { kind: "needsName"; reason: { suggested: string } }
  | { kind: "localHasCommits"; reason: { local: string } };

// `GitRebaseParams`. Without `upstream`, `root` rebases all the commits.
export interface RebaseParams {
  upstream: string | null;
  onto: string | null;
  branch: string | null;
  interactive: boolean;
  rebaseMerges: boolean;
  keepEmpty: boolean;
  root: boolean;
  updateRefs: boolean;
}

export type MergeOutcome =
  | {
      kind: "merged";
      reason: {
        files: number;
        commits: number;
        range: [string, string] | null;
        // With a path filter, how many of the `commits` touch them.
        matching: number | null;
      };
    }
  | { kind: "upToDate" }
  | { kind: "conflicts" }
  | { kind: "localChanges"; reason: { files: string[] } }
  | { kind: "untracked"; reason: { files: string[] } }
  | { kind: "unmerged" }
  | { kind: "failed"; reason: string };

// The merge of the branches popup. With Smart Merge, what was saved and how it came back; `restore` is `null` if it was not
// attempted: with conflicts it is returned after resolving them.
export interface BranchMergeResult {
  outcome: MergeOutcome;
  saved: SavedChanges | null;
  restore: RestoreOutcome | null;
}

export type RestoreOutcome =
  | { kind: "restored" }
  | { kind: "restoredWithoutIndex" }
  | { kind: "conflicts" }
  | { kind: "restoreBlocked"; reason: { files: string[] } }
  | { kind: "gone" }
  | { kind: "failed"; reason: string };

export type UpdateOutcome =
  | { kind: "success" }
  | { kind: "nothingToUpdate" }
  | { kind: "conflicts" }
  | { kind: "notReady"; reason: string }
  | { kind: "noTrackedBranch"; reason: { branch: string } }
  | { kind: "error"; reason: string }
  | { kind: "cancelled" }
  // Asked before saving anything, for `branch`.
  | { kind: "rebasingMergeCommits"; reason: { branch: string } }
  // Asked before saving anything. `protected` is empty if it could not be read.
  | { kind: "rebasingPublishedCommits"; reason: { branch: string; protected: string } };

export interface UpdateResult {
  outcome: UpdateOutcome;
  title: string | null;
  files: number;
  commits: number;
  range: [string, string] | null;
  // With a path filter, how many of the `commits` touch them.
  matching: number | null;
  saved: SavedChanges | null;
  restore: RestoreOutcome | null;
  inProgress: boolean;
  // Git's output when the submodule update after a good Update failed (decision 7).
  submoduleError: string | null;
}

export interface UpdateOptions {
  target?: string;
  setUpstream?: boolean;
  fetched?: boolean;
  // The branch for which `Rebase` was answered to `Rebasing Merge Commits`.
  rebaseMerges?: string;
  // The branch for which `Rebase Anyway` was answered to `Rebasing Published Commit`.
  publishedConfirmed?: string;
}

export type PullOption = "rebase" | "ffOnly" | "noFf" | "squash" | "noCommit" | "noVerify";

// Remote branches go with their remote in front: `origin/main`.
export interface PullInfo {
  branch: string | null;
  remotes: string[];
  remoteBranches: string[];
  tracked: [string, string] | null;
}

export type PushTags = "all" | "currentBranch";
export type ForceMode = "lease" | "plain";

// Without `remote` there is none defined.
export interface PushTarget {
  remote: string | null;
  branch: string;
  isNew: boolean;
}

// `branch`: the one being pushed, if it is not a detached HEAD. Empty `head`: repository without commits.
export interface PushInfo {
  repository: string;
  branch: string | null;
  head: string | null;
  remotes: string[];
  remoteBranches: string[];
  target: PushTarget;
  prePushHook: boolean;
  protectedBranches: string[];
  forcePushTargets: string[];
}

export type CommitSummary = LostCommit;

// `source`: a branch or the hash of a commit.
export interface PushSpec {
  source: string;
  remote: string;
  target: string;
  tags: PushTags | null;
  force: ForceMode | null;
  noVerify: boolean;
}

export type PushStatus =
  | { kind: "success" | "newBranch" | "upToDate" | "forced" | "rejectedNoFf" | "rejectedStale" }
  | { kind: "rejectedOther" | "error"; reason: string };

export interface PushResult {
  status: PushStatus;
  pushed: number;
  source: string;
  destination: string;
  tags: string[];
  remote: string;
  // A rejection that is resolved by updating. `merges`: there are unpushed merges.
  updatable: { branch: string; merges: boolean } | null;
}

// `update`: the last Update, with what was received in all. `attempts`: the pushes made.
export interface UpdatedPush {
  push: PushResult;
  update: UpdateResult | null;
  attempts: number;
}

export type Aborted = "merge" | "rebase" | "cherryPick" | "revert" | "squash" | "nothing";

export interface FetchResult {
  pruned: string[];
  failed: { remote: string; error: string }[];
}

export interface RemoteChoice {
  name: string | null;
  // The name offered instead of the remote branch's own (a pull request's `fork/<branch>`).
  suggested?: string | null;
  dropCommits: boolean;
  mode: CheckoutMode;
}

export interface NewBranch {
  name: string;
  start: string | null;
  checkout: boolean;
  overwrite: Overwrite;
  mode: CheckoutMode;
}

export interface DeletedBranch {
  name: string;
  tip: string;
  upstream: string | null;
  forced: boolean;
  base: string | null;
}

export interface DeletedRemote {
  local: DeletedBranch[];
  failures: string[];
}

export interface DeletedTag {
  name: string;
  tip: string;
}

export interface LostCommit {
  hash: string;
  subject: string;
  author: string;
  time: number;
}

// Each one launches a task: the result arrives with `taskResult`.
export const branchCheckout = (target: string, mode: CheckoutMode) =>
  invoke<number>("branch_checkout", { target, mode });
export const branchCheckoutRemote = (remoteBranch: string, choice: RemoteChoice) =>
  invoke<number>("branch_checkout_remote", { remoteBranch, ...choice });
// The result of the task is a `RebaseRun`.
export const branchRebaseOnto = (startPoint: string, branch: string) =>
  invoke<number>("branch_rebase_onto", { startPoint, branch });
// The result of the task is a `BranchMergeResult`.
export const branchMerge = (
  branch: string,
  kind: "local" | "remote" | "tag",
  smart: boolean,
  expectedHead: string,
) => invoke<number>("branch_merge", { branch, kind, smart, expectedHead });
// The result of the task is a `RebaseRun`.
export const branchRebase = (onto: string, branch: string | null, expectedHead: string) =>
  invoke<number>("branch_rebase", { onto, branch, expectedHead });
// The result of the task is a `RemoteRebase`.
export const branchRebaseRemote = (
  remoteBranch: string,
  name: string | null,
  dropCommits: boolean,
  expectedHead: string,
) => invoke<number>("branch_rebase_remote", { remoteBranch, name, dropCommits, expectedHead });
// The result of the task is the remote protected branch that already has commits the rebase
// would rewrite, or `null`. Without `branch`, the current one.
export const rebasePublished = (upstream: string | null, branch: string | null) =>
  invoke<number>("rebase_published", { upstream, branch });
// The result of the task is an `UpdateResult`.
export const branchPullInto = (remoteBranch: string, method: UpdateMethod) =>
  invoke<number>("branch_pull_into", { remoteBranch, method });
// The results of the tasks are a `MergeOutcome` and a `RebaseRun`. `expectedHead` is the HEAD the dialog
// saw.
export const gitMerge = (
  branch: string,
  options: MergeOption[],
  message: string | null,
  expectedHead: string,
) => invoke<number>("git_merge", { branch, options, message, expectedHead });
export const gitRebase = (params: RebaseParams, expectedHead: string) =>
  invoke<number>("git_rebase", { params, expectedHead });
// The result of the task is the names of the branches that are not yet in HEAD.
export const unmergedBranches = () => invoke<number>("unmerged_branches");
// The result of the task is a `merged` `MergeOutcome`: what a merge with conflicts brought in, already
// resolved and committed, from `beforeHead`.
export const mergeSummary = (branch: string, beforeHead: string) =>
  invoke<number>("merge_summary", { branch, beforeHead });
export const revisionsValid = (revisions: string[]) =>
  invoke<boolean[]>("revisions_valid", { revisions });
export const mergeSettingsSet = (branch: string | null, options: MergeOption[]) =>
  invoke<void>("merge_settings_set", { branch, options });
export const rebaseSettingsSet = (options: RebaseFlag[], newBase: string | null) =>
  invoke<void>("rebase_settings_set", { options, newBase });
export const operationContinue = () => invoke<number>("operation_continue");
export const operationSkip = () => invoke<number>("operation_skip");
// The result of the task is an `Aborted`. `expectedHead` is that of `RepoState` when it was asked.
export const operationAbort = (kind: Operation, expectedHead: string) =>
  invoke<number>("operation_abort", { kind, expectedHead });
export type SideStatus = "modified" | "deleted";
// The texts of `conflicts.type.*`.
export type ConflictKind =
  | "both deleted"
  | "both added"
  | "both modified"
  | "deleted by you"
  | "deleted by them"
  | "added by you"
  | "added by them";

// Already in the user's terms: in a rebase, `yours` is the branch being rebased.
export interface Conflict {
  path: string;
  yours: SideStatus;
  theirs: SideStatus;
  kind: ConflictKind;
}

export interface ConflictSession {
  conflicts: Conflict[];
  reversed: boolean;
  yoursColumn: string;
  theirsColumn: string;
  description: string;
}

export type ConflictVersion =
  | { kind: "text"; text: string }
  | { kind: "binary" }
  | { kind: "missing" };

export interface ConflictSides {
  path: string;
  base: ConflictVersion;
  yours: ConflictVersion;
  theirs: ConflictVersion;
  // Left, center and right of the merge window.
  titles: [string, string, string];
  // SHA-256 of the working file when read, or empty if it does not exist.
  hash: string;
  reversed: boolean;
  // With `default`. A missing version counts as empty text; with a binary, empty.
  blocks: MergeBlock[];
  // What `Show Details` of the left and the right title shows: only in a merge, a rebase or a
  // cherry-pick.
  details: [SideDetails | null, SideDetails | null];
}

// One commit, or the commits of `from..to` (full hashes).
export type SideDetails =
  | { kind: "commit"; hash: string }
  | { kind: "range"; from: string; to: string };

// A commit of a range, and whether it touched the conflicted file.
export interface RangeCommit {
  hash: string;
  subject: string;
  conflicted: boolean;
}

export type ConflictSide = "yours" | "theirs";

// How the merge window ended, with the side as the user sees it.
// `abort` and `skip`: the answers of a patch conflict's `Abort…`.
export type MergeResult = "resolved" | "yours" | "theirs" | "cancelled" | "abort" | "skip";

export interface MergeDone {
  path: string;
  result: MergeResult;
}

// What the contents mode of the merge window shows. `path` only picks the syntax highlighting.
export interface MergeContents {
  id: string;
  // The window's title.
  title: string;
  // Left, center and right.
  titles: [string, string, string];
  left: string;
  base: string;
  right: string;
  path: string;
  // A patch conflict: Cancel is `Abort…`, which asks `Abort and Rollback`, `Skip` or `Continue
  // Resolve`.
  abort?: boolean;
}

// `content` is the result, with `\n` line breaks, unless it was cancelled: Accept Left and Accept Right
// give that side's text.
export interface ContentsMergeDone {
  id: string;
  result: MergeResult;
  content?: string;
}

// The working file on disk: `hash` like that of `ConflictSides`, and its dominant line break.
export interface WorkingFile {
  hash: string;
  separator: "\n" | "\r\n" | "\r";
  // Still unresolved in the index.
  conflicted: boolean;
}

// `ComparisonPolicy`: `DEFAULT`, `TRIM_WHITESPACES` and `IGNORE_WHITESPACES`.
export type Whitespace = "default" | "trim" | "ignore";
export type MergeBlockKind = "inserted" | "deleted" | "modified" | "conflict";

// A block of the three-way merge. The ranges are of lines, `[from, to)` and from 0, with
// the lines split by `\n` (`"x\n"` has two). `auto` is the text that automatic resolution leaves
// in place of the base lines, without the final line break; only in a conflict that can
// be resolved.
export interface MergeBlock {
  left: [number, number];
  base: [number, number];
  right: [number, number];
  kind: MergeBlockKind;
  leftChanged: boolean;
  rightChanged: boolean;
  auto: string | null;
}

// Each one launches a task; the result arrives with `taskResult` (`ConflictSession`, `ConflictSides`
// or nothing).
export const conflictsSession = () => invoke<number>("conflicts_session");
export const conflictSides = (path: string) => invoke<number>("conflict_sides", { path });
// The id of a task whose result is the `RangeCommit[]` of `from..to`, newest first.
export const mergeRangeCommits = (from: string, to: string, path: string) =>
  invoke<number>("merge_range_commits", { from, to, path });
// `side` is the user's: the backend inverts it only in a rebase.
export const conflictsAccept = (paths: string[], side: ConflictSide) =>
  invoke<number>("conflicts_accept", { paths, side });
// `expectedHash` is the `hash` of `conflictSides`: if the file changed since then, it fails.
export const conflictResolve = (path: string, content: string, expectedHash: string) =>
  invoke<number>("conflict_resolve", { path, content, expectedHash });
// The stash. An entry is named by its `sha`: the backend looks up its `stash@{n}` when it acts and
// refuses one that is gone. Each command launches a task; the result arrives with `taskResult`.
export interface StashEntry {
  sha: string;
  // As git gives them: the commit it was made on, then the index and the untracked files.
  parents: string[];
  // Author time, in milliseconds.
  time: number;
  // The `n` of `stash@{n}` when it was listed.
  index: number;
  // The message without the hash it starts with.
  subject: string;
  branch: string | null;
  // What git wrote: `WIP on main` and the rest of the message.
  prefix: string | null;
  message: string;
}

export interface StashGroup {
  // `Index on main`, `Untracked files on main`.
  title: string;
  sha: string;
  files: ChangedFile[];
}

export interface StashChanges {
  files: ChangedFile[];
  groups: StashGroup[];
}

export type Unstashed =
  | { kind: "applied" }
  | { kind: "indexConflict"; reason: string }
  | { kind: "conflicts" }
  | { kind: "untracked"; reason: { files: string[] } }
  | { kind: "localChanges"; reason: { files: string[] } }
  | { kind: "failed"; reason: string };

export const stashList = () => invoke<number>("stash_list");
export const stashSave = (message: string, keepIndex: boolean) =>
  invoke<number>("stash_save", { message, keepIndex });
// `Stash Files` of the staging area: these paths, untracked ones included.
export const stashPushPaths = (paths: string[]) => invoke<number>("stash_push_paths", { paths });
export const stashUnstash = (
  sha: string,
  pop: boolean,
  reinstateIndex: boolean,
  branch: string | null,
  expectedHead: string,
) => invoke<number>("stash_unstash", { sha, pop, reinstateIndex, branch, expectedHead });
export const stashDrop = (sha: string) => invoke<number>("stash_drop", { sha });
export const stashClear = () => invoke<number>("stash_clear");
export const stashChanges = (sha: string) => invoke<number>("stash_changes", { sha });
// `sha` is the stash's or that of one of its groups.
export const stashFileSides = (sha: string, path: string, oldPath: string | null) =>
  invoke<number>("stash_file_sides", { sha, path, oldPath });

// The shelf (`src-tauri/src/shelf.rs`). A list as the store keeps it: `name` is its folder,
// `description` what the user sees, `dateMs` the epoch milliseconds.
export interface ShelvedBinary {
  before: string | null;
  after: string | null;
  shelved: string | null;
}

export interface ShelvedList {
  name: string;
  description: string;
  dateMs: number;
  recycled: boolean;
  toDelete: boolean;
  deleted: boolean;
  patch: string;
  binaries: ShelvedBinary[];
}

// One change to shelve: HEAD against the whole file as git would store it, or against only the
// chosen hunks and lines (`selection`).
export interface ShelveEntry {
  path: string;
  oldPath: string | null;
  selection: LineSelection[] | null;
}

export interface ShelveGroup {
  name: string;
  entries: ShelveEntry[];
}

// `snapshot` is the copy taken before rolling back; `rollbackError` what could not be rolled back;
// `skipped` the paths the shelf did not store (a folder, a submodule), which were left as they are.
export type Shelved =
  | { kind: "onlyDirectories" }
  | {
      kind: "done";
      list: ShelvedList;
      snapshot: string | null;
      rollbackError: string | null;
      skipped: string[];
    };

// `failed`: the changelists that could not be shelved, each with why.
export interface ShelvedSilently {
  lists: ShelvedList[];
  failed: { name: string; reason: string }[];
  snapshot: string | null;
  rollbackError: string | null;
  skipped: string[];
}

// Tasks whose values are a `Shelved` and a `ShelvedSilently`. `expectedRepo` is the repository the
// changes were read from: with another one open, nothing is done.
export const shelfShelve = (
  entries: ShelveEntry[],
  description: string,
  rollback: boolean,
  expectedRepo: string,
) => invoke<number>("shelf_shelve", { entries, description, rollback, expectedRepo });
export const shelfShelveSilently = (lists: ShelveGroup[], rollback: boolean, expectedRepo: string) =>
  invoke<number>("shelf_shelve_silently", { lists, rollback, expectedRepo });

// The Shelf tab (`src-tauri/src/shelf/tab.rs`). A file of a list goes by the shelf's own paths:
// `before: null` is a new file, `after: null` a deleted one.
export interface ShelfFile {
  before: string | null;
  after: string | null;
  binary: boolean;
}

export interface ShelfChange {
  file: ShelfFile;
  path: string;
  oldPath: string | null;
  status: string;
}

// A list with its changes, or the error its patch gave (`Cannot load patch file: …`).
export interface ShelfEntry {
  list: ShelvedList;
  changes: ShelfChange[];
  error: string | null;
}

// The files chosen in one list.
export interface ListFiles {
  name: string;
  files: ShelfFile[];
}

// What `Undo` of `Shelf Deletion` gives back, with the date each list had.
export interface Restorable {
  name: string;
  dateMs: number;
}

export interface ShelfDeleted {
  restorable: Restorable[];
  files: number;
  lists: number;
  first: string | null;
  // What could not be deleted, each with why; the rest was deleted.
  errors: string[];
}

export interface ImportedPatches {
  lists: ShelvedList[];
  errors: string[];
}

export interface ShelfImported {
  added: number;
  skipped: number;
}

export interface ShelfSides {
  left: Side;
  right: Side;
  titles: [string, string];
}

// Tasks. `expectedRepo` is the repository the tab showed: with another one open, nothing is done.
export const shelfLists = () => invoke<number>("shelf_lists");
export const shelfSides = (name: string, file: ShelfFile, withLocal: boolean) =>
  invoke<number>("shelf_sides", { name, file, withLocal });
export const shelfFindPatches = (paths: string[]) => invoke<number>("shelf_find_patches", { paths });
export const shelfDelete = (lists: string[], files: ListFiles[], expectedRepo: string) =>
  invoke<number>("shelf_delete", { lists, files, expectedRepo });
export const shelfUndoDelete = (lists: Restorable[], expectedRepo: string) =>
  invoke<number>("shelf_undo_delete", { lists, expectedRepo });
export const shelfRestore = (names: string[], expectedRepo: string) =>
  invoke<number>("shelf_restore", { names, expectedRepo });
export const shelfRename = (name: string, description: string, expectedRepo: string) =>
  invoke<number>("shelf_rename", { name, description, expectedRepo });
export const shelfRecycle = (name: string, files: ShelfFile[], expectedRepo: string) =>
  invoke<number>("shelf_recycle", { name, files, expectedRepo });
export const shelfImportPatches = (files: string[], expectedRepo: string) =>
  invoke<number>("shelf_import_patches", { files, expectedRepo });
export const shelfImportIdea = (expectedRepo: string) =>
  invoke<number>("shelf_import_idea", { expectedRepo });
export const shelfImportToShelf = (
  source: PatchInput,
  mapping: PathMapping,
  selected: string[],
  expectedRepo: string,
) => invoke<number>("shelf_import_to_shelf", { source, mapping, selected, expectedRepo });
export const shelfPatchCreate = (
  chosen: ListFiles[],
  options: PatchOptions,
  to: PatchTarget,
  remember: boolean,
  expectedRepo: string,
) => invoke<number>("shelf_patch_create", { chosen, options, to, remember, expectedRepo });
export const shelfSettingsSet = (shelf: ShelfSettings) => invoke<void>("shelf_settings_set", { shelf });

// Cherry-Pick and Revert Commit of the log.
export type ApplyKind = "cherryPick" | "revert";

// Why the sequence stopped; `hash` is the commit that could not be applied.
export type ApplyStop =
  | { kind: "conflicts"; reason: { hash: string } }
  | { kind: "localChanges"; reason: { hash: string; files: string[] } }
  | { kind: "untracked"; reason: { hash: string; files: string[] } }
  | { kind: "failed"; reason: { hash: string; text: string } };

// `remaining`: what was not even attempted, to continue after resolving.
export interface ApplyResult {
  applied: string[];
  skipped: string[];
  stopped: ApplyStop | null;
  remaining: string[];
}

// `empty`: what was resolved left the tree as it was and there was nothing to commit.
export type ApplyFinished = "committed" | "empty";

// The result of the task is an `ApplyResult`. `hashes` go in the log's order, and `expectedHead` is
// the HEAD seen when the action was chosen.
export const applyCommits = (kind: ApplyKind, hashes: string[], expectedHead: string) =>
  invoke<number>("apply_commits", { kind, hashes, expectedHead });
// The result of the task is an `ApplyFinished`.
export const applyCommitAfterConflicts = (kind: ApplyKind, hash: string) =>
  invoke<number>("apply_commit_after_conflicts", { kind, hash });
// Cherry-Pick Selected Changes and Revert Selected Changes: the paths of a commit, against the base
// that the changes panel shows (null in a root commit).
export interface PathsSelection {
  hash: string;
  base: string | null;
  paths: string[];
}

// `conflicts`: `--3way` left the conflict in the index, for the Conflicts dialog.
export type PathsResult =
  | { kind: "applied" }
  | { kind: "conflicts" }
  | { kind: "failed"; reason: string };

// The result of the task is a `PathsResult`. With `reverse` those changes are reverted.
export const applyPaths = (selection: PathsSelection[], reverse: boolean, expectedHead: string) =>
  invoke<number>("apply_paths", { selection, reverse, expectedHead });
// Edit Commit Message, Drop Commits and Squash Commits of the log (`rewrite.rs`).
export type RewriteKind = "reword" | "drop" | "squash";

// `null`: the entry is enabled. `reword` only counts with one commit, and `squash` with two or more.
export interface RewriteMenu {
  // Interactively Rebase from Here…: only counts with one commit.
  rebase: string | null;
  reword: string | null;
  drop: string | null;
  squash: string | null;
  // Undo Commit…, and Fixup… with Squash Into…: only count with one commit.
  uncommit: string | null;
  autosquash: string | null;
}

// The actions on a single commit: Undo Commit…, Fixup… with Squash Into…, and Interactively Rebase from
// Here….
export type SingleAction = "uncommit" | "autosquash" | "rebase";

// The HEAD from before and from after, for the Undo, and what `rebase::Process` counted, as a
// `RebaseRun`.
export interface Rewritten extends RebaseRun {
  oldHead: string;
  newHead: string;
}

export type Undone =
  | { kind: "done" }
  | { kind: "impossible"; reason: string }
  | { kind: "failed"; reason: string };

// Interactively Rebase from Here… (`rewrite.rs`). A whole line, as git writes it or as the
// log generated it.
export interface RebaseEntry {
  action: string;
  hash: string;
  subject: string;
}

// `GitRebaseEntryWithDetails`: the line, the full commit and its whole message.
export interface RebaseCommit extends RebaseEntry {
  full: string;
  message: string;
}

// What comes out of the `Rebasing Commits` dialog: the list and the messages of the `reword`s.
export interface RebasePlan {
  entries: RebaseEntry[];
  messages: { hash: string; message: string }[];
}

// The base of the rebase (the commit's parent) and the rows generated with the log, or `null` if git
// has to generate the list.
export interface InteractiveStart {
  base: string;
  commits: RebaseCommit[] | null;
}

// The result of the task is an `InteractiveStart`.
export const rewriteInteractiveStart = (hash: string) =>
  invoke<number>("rewrite_interactive_start", { hash });
// With `plan`, what the dialog chose; without it, the list goes to the window when git asks for it. The
// result of the task is a `RebaseRun`.
export const rewriteInteractive = (hash: string, plan: RebasePlan | null, expectedHead: string) =>
  invoke<number>("rewrite_interactive", { hash, plan, expectedHead });

// Reads: the result of the task is a `RewriteMenu`, and that of the others, the reason or `null`.
// `hashes` go in the log's order.
export const rewriteMenu = (hashes: string[]) => invoke<number>("rewrite_menu", { hashes });
export const rewriteCheck = (hashes: string[], kind: RewriteKind) =>
  invoke<number>("rewrite_check", { hashes, kind });
export const rewriteCheckSingle = (hash: string, action: SingleAction) =>
  invoke<number>("rewrite_check_single", { hash, action });
// `GitUncommitAction`: the result of the task is the paths of the commit. `rewriteAutosquash` is the
// rebase of `Commit and Rebase…` from `base` (`<hash>~`); its result is a `RebaseRun`.
export const rewriteUncommit = (hash: string, expectedHead: string) =>
  invoke<number>("rewrite_uncommit", { hash, expectedHead });
export const rewriteAutosquash = (base: string) =>
  invoke<number>("rewrite_autosquash", { base });
// `GitNewCommitMessageActionDialog.validate`: HEAD moved or the oldest on a protected branch.
export const rewriteValidate = (hashes: string[], expectedHead: string) =>
  invoke<number>("rewrite_validate", { hashes, expectedHead });
export const rewriteUndoCheck = (oldHead: string, newHead: string) =>
  invoke<number>("rewrite_undo_check", { oldHead, newHead });
// The result of the task is a `Rewritten`; that of `rewriteUndo`, an `Undone`.
export const rewriteReword = (hash: string, message: string, expectedHead: string) =>
  invoke<number>("rewrite_reword", { hash, message, expectedHead });
export const rewriteDrop = (hashes: string[], expectedHead: string) =>
  invoke<number>("rewrite_drop", { hashes, expectedHead });
export const rewriteSquash = (hashes: string[], message: string, expectedHead: string) =>
  invoke<number>("rewrite_squash", { hashes, message, expectedHead });
export const rewriteUndo = (kind: RewriteKind, oldHead: string, newHead: string) =>
  invoke<number>("rewrite_undo", { kind, oldHead, newHead });
// The commit of an already resolved merge (`GitMerger.mergeCommit`): launches a task without a result.
export const mergeCommit = () => invoke<number>("merge_commit");
// The message from `MERGE_MSG` or, if there is none, from `SQUASH_MSG`, for the Commit view.
export const mergeMessage = () => invoke<string | null>("merge_message");
// The blocks with another whitespace policy, without rereading: it does not touch the repository.
export const mergeBlocks = (left: string, base: string, right: string, policy: Whitespace) =>
  invoke<MergeBlock[]>("merge_blocks", { left, base, right, policy });
export const branchCreate = ({ start, ...branch }: NewBranch) =>
  invoke<number>("branch_create", { ...branch, startPoint: start });
export const branchRename = (old: string, name: string) =>
  invoke<number>("branch_rename", { old, new: name });
export const branchDelete = (name: string) => invoke<number>("branch_delete", { name });
export const branchRestore = (deleted: DeletedBranch) =>
  invoke<number>("branch_restore", { deleted });
export const branchDeleteRemote = (remoteBranch: string, alsoLocal: boolean) =>
  invoke<number>("branch_delete_remote", { remoteBranch, alsoLocal });
export const branchReset = (target: string, mode: ResetMode, how: CheckoutMode) =>
  invoke<number>("branch_reset", { target, mode, how });
export const tagCreate = (name: string, target: string, message: string, force: boolean) =>
  invoke<number>("tag_create", { name, target, message, force });
export const tagDelete = (name: string) => invoke<number>("tag_delete", { name });
export const tagRestore = (deleted: DeletedTag) => invoke<number>("tag_restore", { deleted });
export const fetchAll = () => invoke<number>("fetch_all");
export const fetchRemote = (remote: string) => invoke<number>("fetch_remote", { remote });
export const pullInfo = () => invoke<PullInfo>("pull_info");
export const gitPull = (remote: string, branch: string, options: PullOption[]) =>
  invoke<number>("git_pull", { remote, branch, options });
export const pushInfo = (branch: string | null) => invoke<PushInfo>("push_info", { branch });
export const pushBranchOf = (hash: string) => invoke<string | null>("push_branch_of", { hash });
export const pushOutgoing = (source: string, remote: string, target: string) =>
  invoke<number>("push_outgoing", { source, remote, target });
export const gitPush = (spec: PushSpec) => invoke<number>("git_push", { spec });
export const gitPushUpdate = (
  spec: PushSpec,
  method: UpdateMethod,
  options: UpdateOptions,
  attempts: number,
) => invoke<number>("git_push_update", { spec, method, options, attempts });
export const pushTagsGet = () => invoke<PushTags | null>("push_tags_get");
export const pushTagsSet = (tags: PushTags | null) => invoke<void>("push_tags_set", { tags });
export const repoSettingsGet = () => invoke<RepoSettings>("repo_settings_get");
// The error of the first pattern of the `;`-separated field that does not compile, or `null`.
export const protectedPatternsCheck = (text: string) =>
  invoke<string | null>("protected_patterns_check", { text });
export const cleanTreeSet = (cleanTree: CleanTree) => invoke<void>("clean_tree_set", { cleanTree });
// The user's own patterns, without GitHub's, and `Load branch protection rules from GitHub`.
export const protectedBranchesSet = (patterns: string[], loadGithubProtection: boolean) =>
  invoke<string[]>("protected_branches_set", { patterns, loadGithubProtection });
export const forcePushTargetRemember = (remote: string, branch: string) =>
  invoke<void>("force_push_target_remember", { remote, branch });
export const updateProject = (method: UpdateMethod, options: UpdateOptions) =>
  invoke<number>("update_project", { method, options });
export const updateBranch = (local: string) => invoke<number>("update_branch", { local });
export const updateAbort = () => invoke<number>("update_abort");
export const updateRestore = (saved: SavedChanges) => invoke<number>("update_restore", { saved });
export interface Remote {
  name: string;
  url: string;
  pushUrl: string | null;
}

export interface Cloned {
  path: string;
  // What failed after cloning in full (the submodules); the clone is kept.
  warning: string | null;
}

// `underGit`: the folder is already under Git and the confirmation is missing.
export type InitOutcome = { kind: "created"; path: string } | { kind: "underGit" };

export const remoteList = () => invoke<Remote[]>("remote_list");
// Each one launches a task: the result arrives with `taskResult`.
export const remoteAdd = (name: string, url: string) =>
  invoke<number>("remote_add", { name, url });
export const remoteEdit = (name: string, newName: string, url: string) =>
  invoke<number>("remote_edit", { name, newName, url });
export const remoteRemove = (name: string) => invoke<number>("remote_remove", { name });
export const remoteTestUrl = (url: string, remote: string | null) =>
  invoke<number>("remote_test_url", { url, remote });

// `Configure GPG Key…`. A secret key gpg can sign with, and its first user ID when it has one.
export interface GpgKey {
  id: string;
  userId: string | null;
}

// The repository's effective `commit.gpgSign` and `user.signingkey`, and whether `gpg.format` is
// `ssh`.
export interface SigningConfig {
  sign: boolean;
  key: string | null;
  ssh: boolean;
}

export const gpgConfig = () => invoke<SigningConfig>("gpg_config");
export const gpgKeys = () => invoke<GpgKey[]>("gpg_keys");
// A task: the result arrives with `taskResult`. `null` turns signing off and keeps the key.
export const gpgConfigSet = (key: string | null) => invoke<number>("gpg_config_set", { key });
export const cloneRepository = (url: string, directory: string) =>
  invoke<number>("clone_repository", { url, directory });
export const initRepository = (path: string, confirmed: boolean) =>
  invoke<number>("init_repository", { path, confirmed });
// `false`: the file does not exist and nothing was done.
export const openExcludeFile = () => invoke<boolean>("open_exclude_file");
// Finder with the file selected. `path` must be absolute.
export const revealInFinder = (path: string) => invoke<void>("reveal_in_finder", { path });
// The browser, for an `https://` or `http://` URL only.
export const openUrl = (url: string) => invoke<void>("open_url", { url });

// A GitHub account of Settings > Version Control > GitHub (`src-tauri/src/hosting/commands.rs`).
// `id` is `<server>/<login>`; `server` shows the scheme only when it was typed. `name` is the full
// name GitHub gave, and `error` what went wrong asking for it, with `needsLogin` when the row
// offers `Log in`. `isDefault` is about the open repository.
export interface AccountView {
  id: string;
  server: string;
  login: string;
  name: string | null;
  error: string | null;
  needsLogin: boolean;
  isDefault: boolean;
}

// Every account at once, with no request to GitHub: `name` and `error` are empty, and `needsLogin`
// is set when there is no token. `githubAccountDetails` fills a row in.
export const githubAccounts = () => invoke<AccountView[]>("github_accounts");
// The same rows with no token read, so `needsLogin` is false: on an unsigned build every Keychain
// read can prompt, and the default-account check runs on every repository open.
export const githubAccountsWithoutTokens = () =>
  invoke<AccountView[]>("github_accounts", { tokens: false });
// One account with its details, which take a request to GitHub.
export const githubAccountDetails = (id: string) =>
  invoke<AccountView>("github_account_details", { id });
// A task whose value is the new `AccountView`. A failure says in its `kind` which field of the
// dialog it is about: `login_server` or `login_token`. `editing` is the id of the account a row's `Log in`
// re-logs into: it keeps its server and takes the token's login. The token is sent only here,
// and no command gives it back.
export const githubLogin = (server: string, token: string, editing: string | null) =>
  invoke<number>("github_login", { server, token, editing });
export const githubRemove = (id: string) => invoke<void>("github_remove", { id });
// `Set as Default`, for the open repository.
export const githubSetDefault = (id: string) => invoke<void>("github_set_default", { id });
// The `Generate…` page of the typed server.
export const githubGenerateUrl = (server: string) =>
  invoke<string>("github_generate_url", { server });

// What a place asks a GitHub link for (`src-tauri/src/hosting/links.rs`). `path` is relative to the
// repository root and `lines` are 0-based, the first and the last (the last can be one less than
// the first: 241's column-0 rule). `revision` is the commit of a revision tab; without it the
// file's last commit in HEAD is used.
export type LinkTarget =
  | { kind: "commit"; hash: string }
  | { kind: "file"; path: string; lines?: [number, number] | null; revision?: string | null };
// One GitHub repository of the remotes; `name` is what the submenu shows.
export interface LinkReference {
  name: string;
  url: string;
}
// The links of a target, empty when the place gets none. It asks GitHub nothing.
export const githubLinks = (target: LinkTarget) =>
  invoke<LinkReference[]>("github_links", { target });

// The Pull Requests window (`src-tauri/src/hosting/pulls/mod.rs`). A remote that points to a GitHub
// repository: `display` is what the repository combo shows, `server` the server as an account
// writes it, and `repository` is `<server>/<owner>/<name>`, as the connect error names it.
export interface MappingView {
  remote: string;
  url: string;
  display: string;
  server: string;
  repository: string;
}
// The last successful connect: the remote's URL and the account's id.
export interface PullsChoice {
  url: string;
  account: string;
}
// The mappings in the combo's order, every account by login, and the saved choice.
export interface Known {
  mappings: MappingView[];
  accounts: AccountView[];
  saved: PullsChoice | null;
}
export interface GithubServerPath {
  useHttp: boolean | null;
  host: string;
  port: number | null;
  suffix: string | null;
}
export interface RepoCoordinates {
  server: GithubServerPath;
  owner: string;
  name: string;
}
export interface GithubRepository {
  id: string;
  owner: string;
  nameWithOwner: string;
  viewerPermission: string | null;
  mergeCommitAllowed: boolean;
  squashMergeAllowed: boolean;
  rebaseMergeAllowed: boolean;
  defaultBranch: string | null;
  isFork: boolean;
  url: string;
  sshUrl: string;
}
// An open connection. `coordinates` are the repository path GitHub gave (it may have been renamed
// since the remote was set up); `title` is the list tab's name.
export interface Connection {
  remote: string;
  url: string;
  account: string;
  coordinates: RepoCoordinates;
  viewerLogin: string;
  repository: GithubRepository;
  title: string;
}
// What the window starts from. It asks GitHub nothing.
export const githubKnown = () => invoke<Known>("github_known");
// A task whose value is the `Connection`: `remote` is the remote's name, `account` the account's id.
export const githubConnect = (remote: string, account: string) =>
  invoke<number>("github_connect", { remote, account });
// `Change Repository or Account…`: the saved choice is forgotten.
export const githubForgetConnection = () => invoke<void>("github_forget_connection");

// The pull request list (`src-tauri/src/hosting/pulls/list.rs`), inside the open connection.
export type PullStateFilter = "OPEN" | "CLOSED" | "MERGED";
export type ReviewFilter =
  | "NO_REVIEW"
  | "REQUIRED"
  | "APPROVED"
  | "CHANGES_REQUESTED"
  | "REVIEWED_BY_ME"
  | "NOT_REVIEWED_BY_ME"
  | "AWAITING_REVIEW";
// The free text and the filters; `null` is not set.
export interface SearchValue {
  text: string | null;
  state: PullStateFilter | null;
  assignee: string | null;
  review: ReviewFilter | null;
  author: string | null;
  label: string | null;
}
// The last search (`null` until there is one) and the history, oldest first.
export interface SavedSearch {
  last: SearchValue | null;
  history: SearchValue[];
}
// `color` is six hex digits without `#`, or `null`.
export interface PullRequestLabel {
  name: string;
  color: string | null;
}
export type ReviewerState = "ACCEPTED" | "WAIT_FOR_UPDATES" | "NEED_REVIEW";
// `name` is the presentable name: the full name, or else the login.
export interface PullRequestReviewer {
  name: string;
  state: ReviewerState;
}
// A row of the list. `author` is the login (`null` for a deleted account), `assignees` are
// presentable names, `unresolved` the review threads neither resolved nor outdated, and
// `createdAt` GitHub's ISO 8601 time. Every text is GitHub's: it is shown as text.
export interface PullRequestShort {
  number: number;
  title: string;
  url: string;
  state: "OPEN" | "CLOSED" | "MERGED";
  isDraft: boolean;
  mergeable: "MERGEABLE" | "CONFLICTING" | "UNKNOWN";
  author: string | null;
  createdAt: string;
  labels: PullRequestLabel[];
  assignees: string[];
  reviewers: PullRequestReviewer[];
  unresolved: number;
}
export interface PullRequestPage {
  items: PullRequestShort[];
  endCursor: string | null;
  hasNext: boolean;
}
export type ChooserKind = "collaborators" | "assignees" | "labels";
// A task whose value is the `PullRequestPage`; `cursor` is the previous page's `endCursor`.
export const githubPrPage = (value: SearchValue, cursor: string | null) =>
  invoke<number>("github_pr_page", { value, cursor });
// The update check's ETag, or `null`.
export const githubPrEtag = () => invoke<string | null>("github_pr_etag");
// A task whose value is the logins of the collaborators or the assignees, or the label names.
export const githubPrChooser = (kind: ChooserKind) => invoke<number>("github_pr_chooser", { kind });
export const githubPrSearchSaved = () => invoke<SavedSearch>("github_pr_search_saved");
// The search becomes the last one, and with `remember` it goes into the history.
export const githubPrSearchSave = (value: SearchValue, remember: boolean) =>
  invoke<SavedSearch>("github_pr_search_save", { value, remember });

// A pull request's tab (`src-tauri/src/hosting/pulls/details.rs` and `changes.rs`), inside the open
// connection. Every text is GitHub's: it is shown as text.
export interface HeadRepository {
  owner: string;
  isFork: boolean;
  nameWithOwner: string;
  url: string;
  sshUrl: string;
}
// `body` is GitHub's Markdown, shown as plain text; `reviewers` are in the map's order.
export interface PullRequestDetails {
  number: number;
  url: string;
  title: string;
  body: string;
  state: "OPEN" | "CLOSED" | "MERGED";
  isDraft: boolean;
  baseRefName: string;
  baseRefOid: string;
  headRefName: string;
  headRefOid: string;
  headRepository: HeadRepository | null;
  reviewers: PullRequestReviewer[];
}
export type CiJobState = "FAILED" | "PENDING" | "SKIPPED" | "SUCCESS";
// A status context or a check run of the last commit; `url` is GitHub's.
export interface CiJob {
  name: string;
  state: CiJobState;
  isRequired: boolean;
  url: string | null;
}
// `headRefOid` is the head it was built for; `hasConflicts` is `null` while GitHub does not know.
export interface PullRequestStatus {
  headRefOid: string;
  jobs: CiJob[];
  hasConflicts: boolean | null;
  requiredApprovals: number;
  isRestricted: boolean;
}
// `headline` is the plain headline, `author` the presentable name and `committedDate` ISO 8601.
export interface PullRequestCommit {
  oid: string;
  abbreviatedOid: string;
  headline: string;
  author: string;
  committedDate: string;
  parents: string[];
  url: string;
}
// A changed file and the revisions its diff shows.
export interface ChangeWithSides extends ChangedFile {
  before: string;
  after: string;
}
// `body`: the chosen commit's message body, read from git.
export interface PullRequestChanges {
  changes: ChangeWithSides[];
  body: string | null;
}
// Tasks whose values are the `PullRequestDetails`, the `PullRequestStatus` and the commits.
export const githubPrDetails = (number: number) => invoke<number>("github_pr_details", { number });
export const githubPrStatus = (number: number) => invoke<number>("github_pr_status", { number });
export const githubPrCommits = (number: number) => invoke<number>("github_pr_commits", { number });
// A task whose value is the `PullRequestChanges`: of the whole pull request (`selection` null),
// after fetching the base branch and the head, or of the commit at `selection` in the commits'
// order.
export const githubPrChanges = (
  number: number,
  baseRef: string,
  baseOid: string,
  headOid: string,
  selection: number | null,
) => invoke<number>("github_pr_changes", { number, baseRef, baseOid, headOid, selection });

// What a branch action of a pull request ends in (`src-tauri/src/hosting/pulls/checkout.rs`):
// `gone`, a deleted fork; `noRemote`, no remote for the head repository, with the URLs GitHub
// gave; `fetchFailed`, git's output; else the action's value.
export type Headed<T> =
  | { kind: "gone" }
  | { kind: "noRemote"; urls: string[] }
  | { kind: "fetchFailed"; output: string }
  | ({ kind: "ready" } & T);
export interface CheckedOutBranch {
  branch: string;
  suggested: string;
  outcome: RemoteCheckout;
}
export interface PullRequestRange {
  from: string;
  to: string;
}
// Tasks whose values are a `Headed<CheckedOutBranch>` and a `Headed<PullRequestRange>`.
export const githubPrCheckout = (number: number) => invoke<number>("github_pr_checkout", { number });
export const githubPrShowInLog = (number: number) => invoke<number>("github_pr_show_in_log", { number });
// Whether the current branch is the pull request's, from the details its tab loaded.
export const githubPrCheckedOut = (number: number) => invoke<boolean>("github_pr_checked_out", { number });

// Copy as Patch and Create Patch (`src-tauri/src/patch/create.rs`). A version of a file: `disk` is
// what `git add` would store now, `selection` HEAD with the chosen lines of the disk.
export type PatchVersion =
  | { kind: "head" }
  | { kind: "index" }
  | { kind: "disk" }
  | { kind: "revision"; hash: string }
  | { kind: "selection"; selection: LineSelection[] };

// `before: null` is a new file and `after: null` a deleted one. HEAD and commits are read at
// `oldPath` when there is one, the index and the disk at `path`.
export interface PatchEntry {
  path: string;
  oldPath: string | null;
  before: PatchVersion | null;
  after: PatchVersion | null;
}

// `Base path:` (absolute), `Reverse patch` and the message of the `Subject:` header.
export interface PatchOptions {
  basePath: string;
  reverse: boolean;
  message: string | null;
}

export type PatchTarget = { kind: "file"; path: string } | { kind: "clipboard" };

// What Patch File Settings starts with. `folder: null` is the repository root;
// `showInFinder: null` asks (`SHOW_PATCH_IN_EXPLORER`).
export interface PatchDefaults {
  folder: string | null;
  toClipboard: boolean;
  showInFinder: boolean | null;
}

// The two fields of Patch File Settings with `~` expanded, and whether they exist.
export interface PatchPaths {
  file: string;
  fileExists: boolean;
  base: string;
  baseExists: boolean;
}

// The file written (`null` for the clipboard) and the recoverable copy of a worktree file it
// replaced.
export interface PatchCreated {
  file: string | null;
  snapshot: string | null;
}

// A task whose value is a `PatchCreated`. `remember` saves `To
// clipboard` and the folder, as Patch File Settings does. `expectedRepo` is the repository the
// changes were read from: the patch is made from it even if another one has been opened since.
export const patchCreate = (
  entries: PatchEntry[],
  options: PatchOptions,
  to: PatchTarget,
  remember: boolean,
  expectedRepo: string,
) => invoke<number>("patch_create", { entries, options, to, remember, expectedRepo });
export const patchSettingsGet = (repo: string) =>
  invoke<PatchDefaults>("patch_settings_get", { repo });
export const patchShowInFinderSet = (repo: string, show: boolean) =>
  invoke<void>("patch_show_in_finder_set", { repo, show });
export const patchPaths = (file: string, base: string) =>
  invoke<PatchPaths>("patch_paths", { file, base });

// Apply Patch and Apply Patch from Clipboard (`src-tauri/src/patch_apply.rs`). Each command reads
// the patch again from its source; a relative file path is taken from the repository root.
// A shelved list is read with its stored bases, only `files` when given; its binary files come
// after the patch's files in the plan.
export type PatchInput =
  | { kind: "file"; path: string }
  | { kind: "text"; text: string }
  | { kind: "shelf"; name: string; files: ShelfFile[] | null };

// `Remove/Restore Leading Directory` and `Map Base Directory…` per file of the patch, by its index:
// `strip` leading folders taken from its path and `base`, a folder relative to the repository root.
export interface FileMapping {
  strip: number;
  base: string;
}

export interface PathMapping {
  files: FileMapping[];
}

// `missing`: the file to change, move or delete is not there; `missingBase`: its changes do not
// match and there is no base to merge with (it will fail); `exists`: the file to create, or the
// target of a move, is already there; `invalid`: the patch may not touch it (`error` says why).
export type PlannedStatus =
  | "applicable"
  | "alreadyApplied"
  | "missingBase"
  | "missing"
  | "exists"
  | "invalid";

export type PlannedKind = "added" | "modified" | "deleted" | "moved";

// One file of the patch. `path` is the target, by which the dialog chooses it; `hashes` are what
// the plan read (`absent` for no file), sent back so that a file changed since is not written.
export interface PlannedFile {
  index: number;
  path: string;
  before: string | null;
  after: string | null;
  original: string;
  strip: number;
  maxStrip: number;
  kind: PlannedKind;
  binary: boolean;
  base: boolean;
  status: PlannedStatus;
  error: string | null;
  hashes: Record<string, string>;
  // For a shelved list, which of its files this is.
  shelf?: ShelfFile | null;
  // An invalid binary file of a shelved list that HEAD changed since it was shelved: the Unshelve
  // dialog offers `Overwrite` for it.
  overwritable?: boolean;
}

export interface ApplyPlan {
  subject: string | null;
  message: string | null;
  author: { name: string; email: string } | null;
  additionalInfoError: string | null;
  // The patch cannot be read (`Cannot read patch {0}: {1}` is the dialog's); then there are no files.
  readError: string | null;
  files: PlannedFile[];
}

// `ApplyPatchStatus`, from the least to the most severe.
export type ApplyStatus = "skip" | "success" | "alreadyApplied" | "partial" | "failure" | "abort";

// A file for the merge window: the texts with `\n`, and the hash of the local file as it was read.
export interface PatchConflict {
  path: string;
  base: string;
  local: string;
  patched: string;
  localHash: string;
  movedTo: string | null;
  newMode: number | null;
}

// `status` leaves out the conflicts, whose outcomes the caller folds in.
export interface PatchApplyResult {
  snapshot: string | null;
  status: ApplyStatus;
  failed: string[];
  conflicts: PatchConflict[];
  messages: string[];
}

export interface ApplyChoice {
  selected: string[];
  overwrite: string[];
  expected: Record<string, string>;
}

export interface PatchResolved {
  hash: string;
  messages: string[];
}

// `Show Difference` of one file.
export interface PatchPreview {
  before: string;
  after: string;
  titles: [string, string];
}

// The clipboard for Apply Patch from Clipboard. `length` is in UTF-16 units (0: no text); `patch`
// is the text only when it looks like a patch. With a `limit`, a text that long is not looked at
// and a shorter one comes with its `hash`.
export interface ClipboardPatch {
  length: number;
  hash: string | null;
  patch: string | null;
}

// `PATCH_STORAGE_LOCATION` and `DETECT_PATCH_ON_THE_FLY`.
export interface AppPatch {
  folder: string | null;
  detectPatchOnTheFly: boolean;
}

// Tasks: the plan is an `ApplyPlan`, applying a `PatchApplyResult`, a resolved conflict a
// `PatchResolved`, a rollback the id of the copy taken before it, a preview a `PatchPreview`.
// `expectedHead` is the HEAD the dialog saw: if it changed, nothing is written.
export const patchPlan = (source: PatchInput, mapping: PathMapping | null) =>
  invoke<number>("patch_plan", { source, mapping });
export const patchApply = (
  source: PatchInput,
  mapping: PathMapping,
  choice: ApplyChoice,
  expectedHead: string,
) => invoke<number>("patch_apply", { source, mapping, choice, expectedHead });
export const patchConflictResolve = (conflict: PatchConflict, content: string) =>
  invoke<number>("patch_conflict_resolve", {
    path: conflict.path,
    content,
    localHash: conflict.localHash,
    movedTo: conflict.movedTo,
    newMode: conflict.newMode,
  });
export const patchRollback = (snapshot: string) => invoke<number>("patch_rollback", { snapshot });
export const patchPreview = (source: PatchInput, mapping: PathMapping, index: number) =>
  invoke<number>("patch_preview", { source, mapping, index });
export const patchClipboard = (limit: number | null) =>
  invoke<ClipboardPatch>("patch_clipboard", { limit });
export const patchAppSettingsGet = () => invoke<AppPatch>("patch_app_settings_get");
export const patchStorageSet = (folder: string) => invoke<void>("patch_storage_set", { folder });
export const patchOnTheFlySet = (on: boolean) => invoke<void>("patch_on_the_fly_set", { on });
export const branchCurrent = () => invoke<string | null>("branch_current");
export const branchTrackers = (remoteBranch: string) =>
  invoke<string[]>("branch_trackers", { remoteBranch });
export const branchUnmergedCommits = (base: string, tip: string) =>
  invoke<LostCommit[]>("branch_unmerged_commits", { base, tip });
// What changed since the previous signal: `stash`, that the watcher saw `refs/stash` or its reflog
// change in that quiet period; `shelf`, that a command wrote the open repository's shelf. A signal
// from the interface carries no payload, which counts as neither.
export interface RepoChange {
  stash: boolean;
  shelf: boolean;
}
const NOTHING_KNOWN: RepoChange = { stash: false, shelf: false };
export const onRepoChanged = (handler: (change: RepoChange) => void) =>
  listenOwn<RepoChange | null>("repo-changed", (event) => handler({ ...NOTHING_KNOWN, ...event?.payload }));
// Changelists do not go through the watcher: whoever changes them notifies the other window.
export const notifyRepoChanged = () => emitToOwner("repo-changed");

// The diff window chooses lines of a file. `places` says where each chosen hunk is, so that
// it is possible to count what was lost when it no longer exists.
export interface SelectionChange {
  path: string;
  selection: LineSelection[];
  included: boolean;
  places: Record<string, string>;
}

// Who keeps the line checkboxes the diff window shows: nobody (Local Changes in modal mode, which has no
// checkboxes), the non-modal Commit view, or the Commit Changes dialog while it is open.
export type SelectionOwner = "none" | "commit-view" | "dialog";

// What the owner has chosen: the diff window shows it, it does not decide it.
export interface SelectionState {
  owner: SelectionOwner;
  included: string[];
  selections: Record<string, LineSelection[]>;
  lists: Changelists | null;
}

// What a view publishes: the application adds who owns it.
export type PublishedSelection = Omit<SelectionState, "owner">;

export interface LostSelection extends SelectionLost {
  text: string;
}

export const emitSelectionChanged = (change: SelectionChange) =>
  emitToOwner("selection-changed", change);
export const onSelectionChanged = (handler: (change: SelectionChange) => void) =>
  listenOwn<SelectionChange>("selection-changed", (event) => handler(event.payload));
export const emitSelectionState = (state: SelectionState) => emitToOwner("selection-state", state);
export const onSelectionState = (handler: (state: SelectionState) => void) =>
  listenOwn<SelectionState>("selection-state", (event) => handler(event.payload));
export const emitSelectionLost = (lost: LostSelection) => emitToOwner("selection-lost", lost);
export const onSelectionLost = (handler: (lost: LostSelection) => void) =>
  listenOwn<LostSelection>("selection-lost", (event) => handler(event.payload));
export const onSettingsChanged = (handler: (settings: Settings) => void) =>
  listen<Settings>("settings-changed", (event) => handler(event.payload));
export const onOpenSettings = (handler: () => void) => listenOwn("open-settings", handler);
// File > Recent Projects: the chosen repository, and whether its path still exists.
export interface RecentProject {
  path: string;
  exists: boolean;
}
export const onOpenRecent = (handler: (project: RecentProject) => void) =>
  listenOwn<RecentProject>("open-recent", (event) => handler(event.payload));
export const recentProjectsRemove = (path: string) => invoke<void>("recent_projects_remove", { path });
// A folder handed to the running app (`open -a Ramify <folder>`): it goes through the open flow.
export const onOpenPath = (handler: (path: string) => void) =>
  listenOwn<string>("open-path", (event) => handler(event.payload));
// File > Open…: the main window shows the folder chooser.
export const onOpenProjectChooser = (handler: () => void) =>
  listenOwn("open-project-chooser", handler);
// A Git action that the interface outside the window requests, like the Git menu: its id arrives.
export const onGitAction = (handler: (action: string) => void) =>
  listenOwn<string>("git-action", (event) => handler(event.payload));
export const emitDiffReady = () => emitToOwner("diff-ready");
export const emitDiffDrawn = (timing: DiffTiming) => emitToOwner("diff-drawn", timing);
export const onDiffDrawn = (handler: (timing: DiffTiming) => void) =>
  listenOwn<DiffTiming>("diff-drawn", (event) => handler(event.payload));
export const emitComparison = (comparison: StageComparison) =>
  emitToOwner("diff-comparison", comparison);
export const onComparison = (handler: (comparison: StageComparison) => void) =>
  listenOwn<StageComparison>("diff-comparison", (event) => handler(event.payload));
// Closing or quitting could not be done: the diff window has text that could not be saved.
export const onLeaveRefused = (handler: (message: string) => void) =>
  listenOwn<string>("leave-refused", (event) => handler(event.payload));
// The window is about to close, or the app to quit: its commit message draft is saved first.
export const onCommitDraftSave = (handler: () => void) => listenOwn("commit-draft-save", handler);
export const onDiffReady = (handler: () => void) => listenOwn("diff-ready", handler);
export const onDiffOpen = (handler: (path: string) => void) =>
  listenOwn<string>("diff-open", (event) => handler(event.payload));
// Sent to the diff window only (`emit_to`).
export const onDiffRequest = (handler: (request: DiffRequest) => void) =>
  listenOwn<DiffRequest>("diff-request", (event) => handler(event.payload));
// Close: the window hides itself, after saving what was written.
export const onDiffClose = (handler: () => void) => listenOwn("diff-close", handler);
// Another repository is going to replace this one: save and reply with `diffFlushed`.
export const onDiffFlush = (handler: () => void) => listenOwn("diff-flush", handler);
// The app as a whole gained or lost the focus: moving between its own windows is neither. Every
// main window hears it; the payload is the project window that was activated (the owner of the
// window that got the focus), `null` for a window of no project.
export const onAppActivated = (handler: (owner: string | null) => void) =>
  listenOwn<string | null>("app-activated", (event) => handler(event.payload ?? null));
// This project window became the menu's owner: the menu shows another window's state until this
// one says its own again. The payload is the label of the window that got the focus.
export const onMenuOwnerGained = (handler: (focused: string | null) => void) =>
  listenOwn<string | null>("menu-owner-gained", (event) => handler(event.payload ?? null));
export const onAppDeactivated = (handler: () => void) => listenOwn("app-deactivated", handler);
// Closing the merge window is its Cancel.
export const onMergeClose = (handler: () => void) => listenOwn("merge-close", handler);
// Another repository or quitting the app would throw the merge away: it must be answered with `mergeLeft`.
export const onMergeLeave = (handler: () => void) => listenOwn("merge-leave", handler);
// Each mode hears only its own: a conflict comes with `path`, the contents mode with `id`.
export const onMergeDone = (handler: (done: MergeDone) => void) =>
  listenOwn<MergeDone | ContentsMergeDone>("merge-done", (event) => {
    if ("path" in event.payload) handler(event.payload);
  });
export const onContentsMergeDone = (handler: (done: ContentsMergeDone) => void) =>
  listenOwn<MergeDone | ContentsMergeDone>("merge-done", (event) => {
    if ("id" in event.payload) handler(event.payload);
  });

export interface TaskEvent {
  id: number;
  title: string;
  text: string;
  fraction: number | null;
  state: "running" | "done" | "failed" | "cancelled";
  // On "failed" and "cancelled", the kind of the error (the `kind` of ApiError).
  kind: ApiError["kind"] | null;
  // On "done", what the task returned.
  value: unknown;
}

export interface ConsoleLine {
  time: string;
  command: boolean;
  text: string;
}

export type PromptKind =
  | { type: "username"; url: string }
  | { type: "password"; url: string; user: string }
  | { type: "passphrase"; key: string }
  | { type: "sshPassword"; user: string }
  | { type: "hostKey"; text: string };

export interface AskpassPrompt {
  id: number;
  kind: PromptKind;
}

// What git asks to edit: a rebase list or a message. `form` says how to show it: in the
// free text editor (`text`, without the help lines), in the `Rebasing Commits` dialog
// (`commits`: `text` is the `RebaseCommit` list in JSON, and the answer, a `RebasePlan` in JSON)
// or as the `noop` question.
export interface EditorRequest {
  id: number;
  kind: "todo" | "message";
  form: "text" | "commits" | "noop";
  text: string;
}

export const taskCancel = (id: number) => invoke<void>("task_cancel", { id });
// The reply only goes through here on its way to the socket: `null` cancels the git command that asks.
export const askpassAnswer = (id: number, answer: string | null) =>
  invoke<void>("askpass_answer", { id, answer });
// The new text of the file; `null` cancels, and git abandons the command.
export const editorAnswer = (id: number, text: string | null) =>
  invoke<void>("editor_answer", { id, text });
export const consoleLines = () => invoke<ConsoleLine[]>("console_lines");
export const onTaskChanged = (handler: (event: TaskEvent) => void) =>
  listenOwn<TaskEvent>("task-changed", (event) => handler(event.payload));
export const onAskpassPrompt = (handler: (prompt: AskpassPrompt) => void) =>
  listenOwn<AskpassPrompt>("askpass-prompt", (event) => handler(event.payload));
// The git command no longer waits (it was cancelled or died): the question no longer makes sense.
export const onAskpassDismissed = (handler: (id: number) => void) =>
  listenOwn<number>("askpass-dismissed", (event) => handler(event.payload));
export const onEditorRequest = (handler: (request: EditorRequest) => void) =>
  listenOwn<EditorRequest>("editor-request", (event) => handler(event.payload));
// Git's editor no longer waits (the task was cancelled or ended): the dialog for that request is no longer needed.
export const onEditorDismissed = (handler: (id: number) => void) =>
  listenOwn<number>("editor-dismissed", (event) => handler(event.payload));
export const onConsoleLine = (handler: (line: ConsoleLine) => void) =>
  listenOwn<ConsoleLine>("console-line", (event) => handler(event.payload));

// The Terminal tab (`terminal.rs`). The shell's output comes through `output` as raw bytes.
export const terminalOpen = (cols: number, rows: number, output: Channel<ArrayBuffer | number[]>) =>
  invoke<number>("terminal_open", { cols, rows, output });
export const terminalWrite = (id: number, data: string) =>
  invoke<void>("terminal_write", { id, data });
export const terminalResize = (id: number, cols: number, rows: number) =>
  invoke<void>("terminal_resize", { id, cols, rows });
// A program other than the shell runs in the foreground: closing would end it.
export const terminalBusy = (id: number) => invoke<boolean>("terminal_busy", { id });
export const terminalClose = (id: number) => invoke<void>("terminal_close", { id });
// Ends every session of this project window: a page that starts again has lost their views.
export const terminalReset = () => invoke<void>("terminal_reset");
export const onTerminalExited = (handler: (id: number) => void) =>
  listenOwn<number>("terminal-exited", (event) => handler(event.payload));
