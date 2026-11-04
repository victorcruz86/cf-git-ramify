// The Merge… and Rebase… dialogs of the Git menu and what follows their result. Ported from `GitMerge`,
// `GitMergeAction.handleResult`, `GitMergeDialog`, `GitRebaseDialog` and `GitRebaseProcess`
// (notifySuccess); the texts are the ones in `GitBundle` and `VcsBundle`.
import {
  branchList,
  gitMerge,
  gitRebase,
  gitVersion,
  logRefs,
  mergeSummary,
  rebasePublished,
  mergeSettingsSet,
  rebaseSettingsSet,
  repoSettingsGet,
  repoState,
  revisionsValid,
  settingsGet,
  unmergedBranches,
  type ApiError,
  type BranchList,
  type MergeOption,
  type MergeOutcome,
  type RebaseFlag,
  type KeptChanges,
  type RebaseOutcome,
  type RebaseParams,
  type RebaseRun,
  type RepoInfo,
  type RepoSettings,
  type RepoState,
  type SavedChanges,
} from "../lib/api";
import { commitMerge, type ConflictsResult, type ResolveOptions } from "../conflicts/flow";
import { errorText } from "../lib/errors";
import { dismiss, notify } from "../lib/notifications";
import {
  confirmPublished,
  finishRebase,
  rebasedMessage,
  type PublishedRequest,
  type Saving,
} from "../branches/merging";
import { taskResult } from "../lib/tasks";
import { count, matchingBody, updatedTitle } from "../update/flow";
import {
  parseGitVersion,
  rememberedMergeOptions,
  type GitVersion,
} from "./options";

const MERGE_FAILED = "Git Merge failed";
const REBASE_FAILED = "Rebase failed";
// `notification.title.git.operation.failed` with `Merge`.
const LOCAL_CHANGES_TITLE = "Git Merge Failed";
// `warning.your.local.changes.would.be.overwritten.by.merge`.
const LOCAL_CHANGES =
  "Your local changes will be overwritten by merge.\nCommit, stash, or revert them to proceed.";
const UNTRACKED_TITLE = "Untracked Files Prevent Merge";
const UNMERGED_FILES = "Merging is not possible because you have unmerged files.";
const UP_TO_DATE = "All files are up to date";

export interface MergeChoice {
  branch: string;
  options: MergeOption[];
  message: string;
}

export interface RebaseChoice {
  params: RebaseParams;
  // What `GitRebaseSettings.newBase` remembers: the new base if there is `--onto`, and otherwise the upstream.
  newBase: string;
}

export type GitRequest =
  | {
      kind: "merge";
      current: string;
      branches: string[];
      branch: string;
      options: MergeOption[];
      version: GitVersion | null;
      // Those not yet in HEAD, which are loaded separately; `null`: it could not be known.
      unmerged: Promise<string[] | null>;
    }
  | {
      kind: "rebase";
      branches: string[];
      refs: string[];
      upstream: string;
      options: RebaseFlag[];
      version: GitVersion | null;
      validate: (revisions: string[]) => Promise<boolean[]>;
    }
  // `GitUtil.showPathsInDialog`: the files of a notice, with their description.
  | { kind: "files"; title: string; message: string; files: string[] }
  | PublishedRequest;

export interface GitUi {
  // `null` if cancelled.
  ask<T>(request: GitRequest): Promise<T | null>;
  // The Conflicts dialog: what follows a merge with conflicts.
  resolveConflicts(options?: ResolveOptions): Promise<ConflictsResult>;
  // A stopped rebase, in the card of the `src/operation` flow. `saved`: what is still in the stash.
  rebaseStopped(outcome: RebaseOutcome, saved: SavedChanges[]): void;
  // A rebase that has just stopped on conflicts: opens Conflicts and, if they are resolved, continues.
  rebaseConflicts(saved: SavedChanges[]): Promise<void>;
  // What `rebase::Process` tried to give back and did not come back whole: the Update notices.
  restored?: (kept: KeptChanges[], saving: Saving) => void;
  // Opens the log with that range. Without it `View commits` does not appear.
  viewCommits?: (range: [string, string]) => void;
}

export interface GitApi {
  state(): Promise<RepoState>;
  branches(): Promise<BranchList>;
  tags(): Promise<string[]>;
  settings(): Promise<RepoSettings>;
  // The text of `git --version`.
  version(): Promise<string>;
  unmerged(): Promise<string[]>;
  saveMerge(branch: string, options: MergeOption[]): Promise<unknown>;
  merge(
    branch: string,
    options: MergeOption[],
    message: string | null,
    expectedHead: string,
  ): Promise<MergeOutcome>;
  commitMerge(): Promise<void>;
  // What a merge with conflicts that is already committed brought in, from the previous HEAD.
  summary(branch: string, beforeHead: string): Promise<MergeOutcome>;
  validate(revisions: string[]): Promise<boolean[]>;
  saveRebase(options: RebaseFlag[], newBase: string | null): Promise<unknown>;
  rebase(params: RebaseParams, expectedHead: string): Promise<RebaseRun>;
  // The protected remote branch with commits that the rebase would rewrite, or `null`.
  published(upstream: string | null, branch: string | null): Promise<string | null>;
}

const result = async <T>(id: Promise<number>) => taskResult<T>(await id);

const backend: GitApi = {
  state: repoState,
  branches: branchList,
  tags: async () => (await logRefs()).filter(({ kind }) => kind === "tag").map(({ name }) => name),
  settings: repoSettingsGet,
  version: async () => gitVersion((await settingsGet()).gitPath),
  unmerged: () => result<string[]>(unmergedBranches()),
  saveMerge: mergeSettingsSet,
  merge: (branch, options, message, head) => result(gitMerge(branch, options, message, head)),
  commitMerge,
  summary: (branch, head) => result(mergeSummary(branch, head)),
  validate: revisionsValid,
  saveRebase: rebaseSettingsSet,
  rebase: (params, head) => result(gitRebase(params, head)),
  published: (upstream, branch) => result(rebasePublished(upstream, branch)),
};

// `GitMergeDialog.updateBranchesField`: the remembered branch if it is still there; otherwise the one named like the
// branch followed by the current one (`nameForRemoteOperations`), and otherwise the first.
export function chooseMergeBranch(
  remembered: string | null,
  branches: string[],
  tracked: string | null,
): string {
  if (remembered && branches.includes(remembered)) return remembered;
  const name = tracked === null ? null : tracked.slice(tracked.indexOf("/") + 1);
  return branches.find((branch) => branch === name) ?? branches[0] ?? "";
}

export { rebasedMessage };

// The flags in the order of `GitRebaseOption`.
function rebaseFlags(params: RebaseParams): RebaseFlag[] {
  const on: [RebaseFlag, boolean][] = [
    ["rebaseMerges", params.rebaseMerges],
    ["keepEmpty", params.keepEmpty],
    ["root", params.root],
    ["interactive", params.interactive],
  ];
  return on.filter(([, enabled]) => enabled).map(([flag]) => flag);
}

export function gitFlow(ui: GitUi, api: GitApi = backend) {
  let running = false;
  // Those that act on the open repository: in another one, their buttons would make no sense.
  const cards = new Set<number>();
  // Goes up when the repository changes: what finishes afterwards belonged to the previous one.
  let generation = 0;

  // A cancelled one is not a failure: whoever cancelled already knows.
  async function attempt<T>(failure: string, work: () => Promise<T>): Promise<T | null> {
    try {
      return await work();
    } catch (error) {
      if ((error as ApiError).kind !== "cancelled") {
        notify({ kind: "error", title: failure, body: errorText(error) });
      }
      return null;
    }
  }

  // One at a time: with a dialog open, another request does not open another on top.
  async function exclusive(failure: string, work: () => Promise<void>) {
    if (running) return;
    running = true;
    try {
      await work();
    } catch (error) {
      notify({ kind: "error", title: failure, body: errorText(error) });
    } finally {
      running = false;
    }
  }

  const card = (notification: Parameters<typeof notify>[0]) => cards.add(notify(notification));

  // `GitMerge.update` and `GitRebase.update`: only with the repository in the normal state. The menu already
  // turns them off; this is in case the action arrives some other way.
  const normal = (state: RepoState) => !state.operation && state.branch !== null;

  async function showFiles(title: string, message: string, files: string[]) {
    await ui.ask<void>({ kind: "files", title, message, files });
  }

  function notifyMerged({ files, commits, range, matching }: MergeOutcomeMerged) {
    if (commits > 0) {
      const { viewCommits } = ui;
      card({
        kind: "info",
        title: updatedTitle(files, commits),
        body: matchingBody(matching),
        actions:
          viewCommits && range
            ? [{ label: "View commits", run: () => viewCommits(range) }]
            : undefined,
      });
    } else if (files > 0) {
      // `--no-commit` and `--squash` receive no commits, but leave changed files.
      notify({ kind: "info", title: `${count(files, "file")} updated` });
    } else {
      notify({ kind: "info", title: UP_TO_DATE });
    }
  }

  // It is a datum of the notice: if it cannot be computed, the merge is already done and nothing more is said.
  async function summarize(branch: string, beforeHead: string) {
    const outcome = await api.summary(branch, beforeHead).catch(() => null);
    if (outcome?.kind === "merged") notifyMerged(outcome.reason);
  }

  // `GitMergeAction.handleResult`.
  async function showMerge(
    outcome: MergeOutcome,
    options: MergeOption[],
    branch: string,
    beforeHead: string,
  ) {
    switch (outcome.kind) {
      case "merged":
        notifyMerged(outcome.reason);
        break;
      case "upToDate":
        notify({ kind: "info", title: UP_TO_DATE });
        break;
      case "conflicts": {
        // `proceedAfterAllMerged`: the merge commit, except with `--no-commit`. Afterwards the case joins
        // the success (`result.success() || mergeConflict.hasHappened()`): the summary of what was received appears.
        const commit = !options.includes("noCommit");
        await ui.resolveConflicts({
          onresolved: commit
            ? async () => {
                await api.commitMerge();
                await summarize(branch, beforeHead);
              }
            : undefined,
        });
        break;
      }
      case "localChanges": {
        const { files } = outcome.reason;
        card({
          kind: "error",
          title: LOCAL_CHANGES_TITLE,
          body: LOCAL_CHANGES,
          actions: [
            {
              label: "View them",
              run: () =>
                void showFiles("Local Changes Prevent from Merge", LOCAL_CHANGES, files),
            },
          ],
        });
        break;
      }
      case "untracked": {
        const { files } = outcome.reason;
        card({
          kind: "error",
          title: UNTRACKED_TITLE,
          body: "Move or commit them before Merge",
          actions: [
            {
              label: "View Files…",
              run: () =>
                void showFiles(
                  "Untracked Files Preventing Merge",
                  "These untracked files will be overwritten by Merge",
                  files,
                ),
            },
          ],
        });
        break;
      }
      case "unmerged":
        notify({ kind: "error", title: MERGE_FAILED, body: UNMERGED_FILES });
        break;
      case "failed":
        notify({ kind: "error", title: MERGE_FAILED, body: outcome.reason });
        break;
    }
  }

  // `Git.Merge`.
  function merge(): Promise<void> {
    return exclusive(MERGE_FAILED, async () => {
      const started = generation;
      const state = await attempt(MERGE_FAILED, api.state);
      if (!state || !normal(state)) return;
      const current = state.branch as string;
      const loaded = await attempt(MERGE_FAILED, () =>
        Promise.all([api.branches(), api.settings(), api.version().catch(() => "")]),
      );
      if (!loaded) return;
      const [list, saved, versionText] = loaded;
      const version = parseGitVersion(versionText);
      const branches = [...list.local.map(({ name }) => name), ...list.remote];
      const tracked = list.local.find(({ name }) => name === current)?.tracked ?? null;
      const choice = await ui.ask<MergeChoice>({
        kind: "merge",
        current,
        branches,
        branch: chooseMergeBranch(saved.merge.branch, branches, tracked),
        options: rememberedMergeOptions(saved.merge.options, version),
        version,
        unmerged: api.unmerged().catch(() => null),
      });
      if (!choice || generation !== started) return;
      // `doOKAction`: the branch and the options are remembered on accepting; the message is not.
      await api.saveMerge(choice.branch, choice.options).catch(() => {});
      const message = choice.options.includes("message") ? choice.message.trim() || null : null;
      const outcome = await attempt(MERGE_FAILED, () =>
        api.merge(choice.branch, choice.options, message, state.head),
      );
      if (outcome && generation === started) await showMerge(outcome, choice.options, choice.branch, state.head);
    });
  }

  // If the rebase did not get to start (a dirty tree, a target that does not exist), there is nothing to
  // continue or abort: the card of task 2 would offer `Retry` and `Abort` in vain.
  async function rebaseIsOngoing(): Promise<boolean> {
    try {
      return (await api.state()).operation === "rebasing";
    } catch {
      return true;
    }
  }

  async function notifyRebased(params: RebaseParams, before: RepoState) {
    let branch: string | null = null;
    try {
      branch = (await api.state()).branch;
    } catch {
      // It is a datum of the notice: without it, only the name is missing.
    }
    // `GitRebaseProcess.notifySuccess`: the base is the new one, or the upstream if there is no `--onto`.
    let base = params.upstream ? (params.onto ?? params.upstream) : params.onto;
    if (base === "HEAD") base = before.branch;
    notify({
      kind: "info",
      title: "Rebase successful",
      body: rebasedMessage(branch, base, params.branch !== null),
    });
  }

  async function showRebase(run: RebaseRun, params: RebaseParams, before: RepoState) {
    const base = params.onto ?? params.upstream ?? "HEAD";
    await finishRebase(
      run,
      ui,
      { operation: "rebase", from: base },
      () => notifyRebased(params, before),
      rebaseIsOngoing,
    );
  }

  // `Git.Rebase`.
  function rebase(): Promise<void> {
    return exclusive(REBASE_FAILED, async () => {
      const started = generation;
      const state = await attempt(REBASE_FAILED, api.state);
      if (!state || !normal(state)) return;
      const loaded = await attempt(REBASE_FAILED, () =>
        Promise.all([
          api.branches(),
          api.tags().catch((): string[] => []),
          api.settings(),
          api.version().catch(() => ""),
        ]),
      );
      if (!loaded) return;
      const [list, tags, saved, versionText] = loaded;
      const locals = list.local.map(({ name }) => name);
      // `loadSettings`: the remembered base only if git still resolves it.
      const base = saved.rebase.newBase;
      const valid = base ? await api.validate([base]).catch(() => [false]) : [false];
      const choice = await ui.ask<RebaseChoice>({
        kind: "rebase",
        branches: locals,
        refs: [...locals, ...list.remote, ...tags],
        upstream: base && valid[0] ? base : "",
        options: saved.rebase.options,
        version: parseGitVersion(versionText),
        validate: api.validate,
      });
      if (!choice || generation !== started) return;
      const { params } = choice;
      await api.saveRebase(rebaseFlags(params), choice.newBase || null).catch(() => {});
      // `checkForRebasingPublishedCommits`, with the branch being rebased or the current one.
      const ask = (request: PublishedRequest) => ui.ask<boolean>(request);
      const rebased = params.branch ?? state.branch;
      if (!(await confirmPublished(ask, api.published, params.upstream, rebased))) return;
      if (generation !== started) return;
      const run = await attempt(REBASE_FAILED, () => api.rebase(params, state.head));
      if (run && generation === started) await showRebase(run, params, state);
    });
  }

  function repositoryChanged(_info?: Pick<RepoInfo, "path">) {
    generation += 1;
    cards.forEach(dismiss);
    cards.clear();
  }

  return { merge, rebase, repositoryChanged };
}

type MergeOutcomeMerged = Extract<MergeOutcome, { kind: "merged" }>["reason"];
