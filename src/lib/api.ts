import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";

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

export interface Settings {
  gitPath: string | null;
  stagingArea: boolean;
  warnCrlf: boolean;
  warnDetached: boolean;
  // `Warn when committing files larger than [limit] MB`.
  warnLargeFiles: boolean;
  largeFilesLimitMb: number;
  warnBadFileNames: boolean;
  signCommits: boolean;
  signingKey: string | null;
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
}

export type CommitMode = "modal" | "non-modal";

export const commitMode = (settings: Settings): CommitMode =>
  settings.nonModalCommit ? "non-modal" : "modal";

// `Enable staging area` only counts in the non-modal interface (`canEnableStagingArea`): while modal it
// stays saved but is not in effect.
export const effectiveStaging = (settings: Settings): boolean =>
  settings.nonModalCommit && settings.stagingArea;

// `GitMergeOption`, in their order.
export type MergeOption = "noFf" | "ffOnly" | "squash" | "message" | "noCommit" | "noVerify";

// The flags remembered by the Rebase… dialog, which are those of `REBASE_FLAGS`.
export type RebaseFlag = "rebaseMerges" | "keepEmpty" | "root" | "interactive";

// Per repository. `forcePushTargets`: the `Don't warn about this target` ones, as `remote:branch`.
export interface RepoSettings {
  protectedBranches: string[];
  forcePushTargets: string[];
  // `GitMergeSettings`: the branch and the options of the last Merge…, without the message.
  merge: { branch: string | null; options: MergeOption[] };
  // `GitRebaseSettings`: the flags and the base (the new one if there was `--onto`, otherwise the upstream).
  rebase: { options: RebaseFlag[]; newBase: string | null };
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
export const diffFlushed = (saved: boolean) => invoke<void>("diff_flushed", { saved });
// Decision 3: with the merge window open, it brings it to the front with the file it already had.
export const mergeWindowOpen = (path: string) => invoke<void>("merge_window_open", { path });
export const mergeWindowPath = () => invoke<string | null>("merge_window_path");
// Finishes the merge window: the main one receives `merge-done` and the window is destroyed.
export const mergeWindowDone = (result: MergeResult) =>
  invoke<void>("merge_window_done", { result });
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
export const snapshotsRestore = (id: string) => invoke<string>("snapshots_restore", { id });
export const perfEnabled = () => invoke<boolean>("perf_enabled");
export const perfReport = (json: string) => invoke<void>("perf_report", { json });
export const appVersion = () => invoke<string>("app_version");
export const initialRepo = () => invoke<string | null>("initial_repo");
export const showWindow = () => invoke<void>("show_window");
export const hideWindow = () => invoke<void>("hide_window");
export const repoOpen = (repo: string) => invoke<RepoInfo>("repo_open", { repo });
export const repoState = () => invoke<RepoState>("repo_state");
// Turns on in the Git menu the entries of the operation in progress that apply, and turns off the others.
export const gitMenuEnable = (ids: string[]) => invoke<void>("git_menu_enable", { ids });
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
  listen("branch-sync-changed", () => handler());
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
export const logRecentFilters = () => invoke<RecentFilters>("log_recent_filters");
export const logRememberFilter = (filter: string, values: string[]) =>
  invoke<RecentFilters>("log_remember_filter", { filter, values });
export type CheckoutMode = "plain" | "smart" | "force";
export type ResetMode = "soft" | "mixed" | "hard" | "keep";
export type Overwrite = "no" | "try" | "drop";

export interface SavedChanges {
  sha: string;
  message: string;
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
  | { kind: "rebasingMergeCommits" };

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
}

export interface UpdateOptions {
  target?: string;
  setUpstream?: boolean;
  fetched?: boolean;
  rebaseMerges?: boolean;
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
export const branchMerge = (branch: string, smart: boolean, expectedHead: string) =>
  invoke<number>("branch_merge", { branch, smart, expectedHead });
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
}

export type ConflictSide = "yours" | "theirs";

// How the merge window ended, with the side as the user sees it.
export type MergeResult = "resolved" | "yours" | "theirs" | "cancelled";

export interface MergeDone {
  path: string;
  result: MergeResult;
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
// `side` is the user's: the backend inverts it only in a rebase.
export const conflictsAccept = (paths: string[], side: ConflictSide) =>
  invoke<number>("conflicts_accept", { paths, side });
// `expectedHash` is the `hash` of `conflictSides`: if the file changed since then, it fails.
export const conflictResolve = (path: string, content: string, expectedHash: string) =>
  invoke<number>("conflict_resolve", { path, content, expectedHash });
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
// The patterns that are not valid.
export const protectedBranchesSet = (patterns: string[]) =>
  invoke<string[]>("protected_branches_set", { patterns });
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
export const cloneRepository = (url: string, directory: string) =>
  invoke<number>("clone_repository", { url, directory });
export const initRepository = (path: string, confirmed: boolean) =>
  invoke<number>("init_repository", { path, confirmed });
// `false`: the file does not exist and nothing was done.
export const openExcludeFile = () => invoke<boolean>("open_exclude_file");
export const openInNewWindow = (path: string) => invoke<void>("open_in_new_window", { path });
export const branchCurrent = () => invoke<string | null>("branch_current");
export const branchTrackers = (remoteBranch: string) =>
  invoke<string[]>("branch_trackers", { remoteBranch });
export const branchUnmergedCommits = (base: string, tip: string) =>
  invoke<LostCommit[]>("branch_unmerged_commits", { base, tip });
export const onRepoChanged = (handler: () => void) => listen("repo-changed", handler);
// Changelists do not go through the watcher: whoever changes them notifies the other window.
export const notifyRepoChanged = () => emit("repo-changed");

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
  emit("selection-changed", change);
export const onSelectionChanged = (handler: (change: SelectionChange) => void) =>
  listen<SelectionChange>("selection-changed", (event) => handler(event.payload));
export const emitSelectionState = (state: SelectionState) => emit("selection-state", state);
export const onSelectionState = (handler: (state: SelectionState) => void) =>
  listen<SelectionState>("selection-state", (event) => handler(event.payload));
export const emitSelectionLost = (lost: LostSelection) => emit("selection-lost", lost);
export const onSelectionLost = (handler: (lost: LostSelection) => void) =>
  listen<LostSelection>("selection-lost", (event) => handler(event.payload));
export const onSettingsChanged = (handler: (settings: Settings) => void) =>
  listen<Settings>("settings-changed", (event) => handler(event.payload));
export const onOpenSettings = (handler: () => void) => listen("open-settings", handler);
// A Git action that the interface outside the window requests, like the Git menu: its id arrives.
export const onGitAction = (handler: (action: string) => void) =>
  listen<string>("git-action", (event) => handler(event.payload));
export const emitDiffReady = () => emit("diff-ready");
export const emitDiffDrawn = (timing: DiffTiming) => emit("diff-drawn", timing);
export const onDiffDrawn = (handler: (timing: DiffTiming) => void) =>
  listen<DiffTiming>("diff-drawn", (event) => handler(event.payload));
export const emitComparison = (comparison: StageComparison) =>
  emit("diff-comparison", comparison);
export const onComparison = (handler: (comparison: StageComparison) => void) =>
  listen<StageComparison>("diff-comparison", (event) => handler(event.payload));
// Closing or quitting could not be done: the diff window has text that could not be saved.
export const onLeaveRefused = (handler: (message: string) => void) =>
  listen<string>("leave-refused", (event) => handler(event.payload));
export const onDiffReady = (handler: () => void) => listen("diff-ready", handler);
export const onDiffOpen = (handler: (path: string) => void) =>
  listen<string>("diff-open", (event) => handler(event.payload));
// Close: the window hides itself, after saving what was written.
export const onDiffClose = (handler: () => void) => listen("diff-close", handler);
// Another repository is going to replace this one: save and reply with `diffFlushed`.
export const onDiffFlush = (handler: () => void) => listen("diff-flush", handler);
// Closing the merge window is its Cancel.
export const onMergeClose = (handler: () => void) => listen("merge-close", handler);
// Another repository or quitting the app would throw the merge away: it must be answered with `mergeLeft`.
export const onMergeLeave = (handler: () => void) => listen("merge-leave", handler);
export const onMergeDone = (handler: (done: MergeDone) => void) =>
  listen<MergeDone>("merge-done", (event) => handler(event.payload));

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
  listen<TaskEvent>("task-changed", (event) => handler(event.payload));
export const onAskpassPrompt = (handler: (prompt: AskpassPrompt) => void) =>
  listen<AskpassPrompt>("askpass-prompt", (event) => handler(event.payload));
// The git command no longer waits (it was cancelled or died): the question no longer makes sense.
export const onAskpassDismissed = (handler: (id: number) => void) =>
  listen<number>("askpass-dismissed", (event) => handler(event.payload));
export const onEditorRequest = (handler: (request: EditorRequest) => void) =>
  listen<EditorRequest>("editor-request", (event) => handler(event.payload));
// Git's editor no longer waits (the task was cancelled or ended): the dialog for that request is no longer needed.
export const onEditorDismissed = (handler: (id: number) => void) =>
  listen<number>("editor-dismissed", (event) => handler(event.payload));
export const onConsoleLine = (handler: (line: ConsoleLine) => void) =>
  listen<ConsoleLine>("console-line", (event) => handler(event.payload));
