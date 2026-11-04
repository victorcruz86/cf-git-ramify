import {
  forcePushTargetRemember,
  gitPush,
  gitPushUpdate,
  pushBranchOf,
  pushInfo,
  pushOutgoing,
  pushTagsGet,
  pushTagsSet,
  settingsGet,
  settingsSave,
  type ApiError,
  type CommitSummary,
  type PushInfo,
  type PushResult,
  type PushSpec,
  type PushTags,
  type Settings,
  type UpdatedPush,
  type UpdateMethod,
  type UpdateOptions,
  type UpdateResult,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { dismiss, notify, type Notification } from "../lib/notifications";
import { taskResult } from "../lib/tasks";
import { shortHash } from "../log/draw";
import { isValidRefName } from "../branches/refName";
import { branchesOf } from "../update/pullOptions";
import { shouldPreview } from "./commitAndPush";
import { isProtectedBranch } from "./force";

const LOAD_FAILED = "Cannot load remote branches";
const PUSH_FAILED = "Push failed";
const PUSH_REJECTED = "Push rejected";
const FORCE_ANYWAY = "Force push anyway";

// `DEFAULT_CHILDREN_PRESENTATION_NUMBER`: the dots double what is shown.
export const FIRST_COMMITS = 20;

// What goes into the refspec and what is read in the row: `GitPushSource.getRevision` and
// `getPresentation`.
export interface PushSource {
  revision: string;
  label: string;
}

export interface PushChoice {
  remote: string;
  target: string;
  tags: PushTags | null;
  noVerify: boolean;
  force: boolean;
  // The checkbox of `GitPushAfterCommitDialog`.
  preview: boolean;
  // `Don't warn about this target`.
  rememberTarget: boolean;
}

export interface RejectedAnswer {
  method: UpdateMethod;
  remember: boolean;
}

export interface PushRequest {
  kind: "push";
  info: PushInfo;
  source: PushSource;
  tags: PushTags | null;
  outgoing: (remote: string, target: string) => Promise<CommitSummary[]>;
  // The information again, with the remote already defined. `null`: it was not defined.
  defineRemote?: () => Promise<PushInfo | null>;
  // The checkbox of `GitPushAfterCommitDialog`, only after a commit.
  preview?: boolean | null;
}

export type PushDialogRequest =
  | PushRequest
  | { kind: "pushRejected"; branch: string; method: UpdateMethod; merges: boolean }
  | { kind: "rebaseMerges" };

// What it shares with `src/update/flow.ts`: updating after a rejection is one more Update.
export interface PushUpdates {
  // One at a time. `false`: there was another one, and `work` was not launched.
  exclusive(work: () => Promise<void>): Promise<boolean>;
  // `here`: the repository that was updated is still open.
  report(update: UpdateResult, here: boolean): void;
}

export interface PushUi {
  // `null` if cancelled.
  ask<T>(request: PushDialogRequest): Promise<T | null>;
  updates: PushUpdates;
  // `Define remote`. Without it, the link comes out disabled. `true` if it was defined.
  defineRemote?: () => Promise<boolean>;
  // Opens the `Update Info` tab with that range. Without it `View received commits` does not appear.
  viewCommits?: (range: [string, string]) => void;
}

export interface PushApi {
  info(branch: string | null): Promise<PushInfo>;
  // The branch that contains a commit, to push up to it.
  branchOf(hash: string): Promise<string | null>;
  outgoing(source: string, remote: string, target: string): Promise<CommitSummary[]>;
  push(spec: PushSpec): Promise<PushResult>;
  // `attempts`: the pushes made so far.
  updateAndPush(
    spec: PushSpec,
    method: UpdateMethod,
    options: UpdateOptions,
    attempts: number,
  ): Promise<UpdatedPush>;
  // The last choice of this repository.
  tags(): Promise<PushTags | null>;
  saveTags(tags: PushTags | null): Promise<unknown>;
  settings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<unknown>;
  rememberForceTarget(remote: string, branch: string): Promise<unknown>;
}

const backend: PushApi = {
  info: pushInfo,
  branchOf: pushBranchOf,
  outgoing: async (source, remote, target) =>
    taskResult<CommitSummary[]>(await pushOutgoing(source, remote, target)),
  push: async (spec) => taskResult<PushResult>(await gitPush(spec)),
  updateAndPush: async (spec, method, options, attempts) =>
    taskResult<UpdatedPush>(await gitPushUpdate(spec, method, options, attempts)),
  tags: pushTagsGet,
  saveTags: pushTagsSet,
  settings: settingsGet,
  saveSettings: settingsSave,
  rememberForceTarget: forcePushTargetRemember,
};

export const pushTitle = (repository: string) => `Push Commits to ${repository}`;

export function pushSource({ branch, head }: PushInfo, revision?: string): PushSource {
  const commit = revision ?? (branch === null ? head : null);
  if (commit) return { revision: commit, label: shortHash(commit) };
  return { revision: branch ?? "", label: branch ?? "" };
}

export function shownCommits<T>(commits: T[], limit: number) {
  return { shown: commits.slice(0, limit), more: commits.length > limit };
}

// `GitPushTarget.parse`.
export const targetIsNew = ({ remoteBranches }: PushInfo, remote: string, branch: string) =>
  !remoteBranches.includes(`${remote}/${branch}`);

export const targetNames = (info: PushInfo, remote: string | null) => branchesOf(info, remote);

// `GitRefNameValidator`. A leading `+` would be a forced refspec: it does not get past Rust either.
export function targetProblem(branch: string): string | null {
  return isValidRefName(branch) && !branch.startsWith("+")
    ? null
    : `Invalid destination branch name: ${branch}`;
}

export function commitTooltip({ hash, subject, author, time }: CommitSummary): string {
  const date = new Date(time * 1000).toLocaleDateString();
  return `${shortHash(hash)} ${date} by ${author}\n${subject}`;
}

// `{0,choice,1#commit|2#commits}`: zero is also singular.
const commits = (amount: number) => `${amount} ${amount >= 2 ? "commits" : "commit"}`;

// `selectBundleMessageWithTags`.
function withTags(text: string, { tags, remote }: PushResult): string {
  if (tags.length === 0) return text;
  return tags.length === 1
    ? `${text}, and tag ${tags[0]} to ${remote}`
    : `${text}, and ${tags.length} tags to ${remote}`;
}

function upToDate({ tags, remote }: PushResult): string {
  if (tags.length === 0) return "Everything is up to date";
  return tags.length === 1 ? `Pushed tag ${tags[0]} to ${remote}` : `Pushed ${tags.length} tags to ${remote}`;
}

// `formRepoDescription` with the Update's result: after one that ended well, the rejection is another.
function rejectedAfter(update: UpdateResult | null, destination: string): string {
  switch (update?.outcome.kind) {
    case undefined:
    case "success":
    case "nothingToUpdate":
      return `Push to ${destination} was rejected`;
    case "conflicts":
      return "Push was canceled, because not all conflicts were resolved during the update.\nResolve the conflicts and try pushing again";
    case "cancelled":
    case "rebasingMergeCommits":
      return "Push was rejected, and update was canceled";
    default:
      return "Push was rejected, and update failed with an error";
  }
}

const received = (update: UpdateResult | null) =>
  update && update.commits > 0
    ? `${commits(update.commits)} received during the push operation`
    : undefined;

// `GitPushResultNotification.create`, with a single repository.
export function pushNotification(
  result: PushResult,
  update: UpdateResult | null = null,
): Notification {
  const { status, pushed, source, destination } = result;
  const rejected = (body: string): Notification => ({
    kind: "warning",
    title: PUSH_REJECTED,
    body,
  });
  switch (status.kind) {
    case "success":
      return {
        kind: "info",
        title: withTags(`Pushed ${commits(pushed)} to ${destination}`, result),
        body: received(update),
      };
    case "newBranch":
      return {
        kind: "info",
        title: withTags(`Pushed ${source} to new branch ${destination}`, result),
      };
    case "upToDate":
      return { kind: "info", title: upToDate(result), body: received(update) };
    case "forced":
      return { kind: "info", title: `Force pushed ${source} to ${destination}` };
    case "rejectedNoFf":
      return rejected(rejectedAfter(update, destination));
    case "rejectedStale":
      return rejected(`Force-with-lease push ${source} to ${destination} was rejected`);
    case "rejectedOther":
      return rejected(`Push ${source} to ${destination} was rejected by the remote\n${status.reason}`);
    case "error":
      return { kind: "error", title: PUSH_FAILED, body: status.reason };
  }
}

// `GitPushResultNotification`: the notification's action.
function viewCommitsLabel({ commits: amount, matching }: UpdateResult): string {
  const noun = (count: number) => (count >= 2 ? "commits" : "commit");
  return matching
    ? `View ${commits(matching)} matching the filter`
    : `View received ${noun(amount)}`;
}

export function pushFlow(ui: PushUi, api: PushApi = backend) {
  // `expireExistingErrorsAndWarnings`: the previous warning no longer says anything about this push.
  let warning: number | null = null;
  // Goes up when the repository changes. An earlier `Force push anyway` would force on the open one.
  let generation = 0;

  // A cancellation is not a failure: whoever cancelled already knows.
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

  // `ForcePushNotificationAction`: IntelliJ does not ask, and it is only reached with this click.
  function actionsOf(
    spec: PushSpec,
    { status }: PushResult,
    update: UpdateResult | null,
    started: number,
  ): Notification["actions"] {
    if (status.kind === "rejectedStale") {
      return [{ label: FORCE_ANYWAY, run: () => void run({ ...spec, force: "plain" }, started) }];
    }
    const { viewCommits } = ui;
    const range = update?.range;
    return viewCommits && range
      ? [{ label: viewCommitsLabel(update), run: () => viewCommits(range) }]
      : undefined;
  }

  function announce(
    spec: PushSpec,
    result: PushResult,
    update: UpdateResult | null,
    started: number,
  ) {
    const notification = pushNotification(result, update);
    const actions = generation === started ? actionsOf(spec, result, update, started) : undefined;
    const id = notify({ ...notification, actions });
    if (notification.kind !== "info" && generation === started) warning = id;
  }

  // `GitUpdateProcess` with `checkForRebaseOverMergeProblem`: if it was not asked before, it is asked now.
  async function askRebaseOrMerge(
    spec: PushSpec,
    done: UpdatedPush,
    update: UpdateResult,
    started: number,
  ) {
    const asking = generation === started;
    const decision = asking ? await ui.ask<UpdateMethod>({ kind: "rebaseMerges" }) : null;
    if (generation !== started || !decision) {
      const cancelled: UpdateResult = { ...update, outcome: { kind: "cancelled" } };
      announce(spec, done.push, cancelled, started);
    } else if (decision === "merge") {
      await retry(spec, "merge", { fetched: true }, done.attempts, started);
    } else {
      await retry(spec, "rebase", { fetched: true, rebaseMerges: true }, done.attempts, started);
    }
  }

  async function retry(
    spec: PushSpec,
    method: UpdateMethod,
    options: UpdateOptions,
    attempts: number,
    started: number,
  ) {
    const done = await attempt(PUSH_FAILED, () =>
      api.updateAndPush(spec, method, options, attempts),
    );
    if (!done) return;
    const { update, push: result } = done;
    // Before asking: what an earlier Update left behind does not come back with the next one.
    if (update) ui.updates.report(update, generation === started);
    if (update?.outcome.kind === "rebasingMergeCommits") {
      await askRebaseOrMerge(spec, done, update, started);
      return;
    }
    announce(spec, result, update, started);
  }

  // `savePushUpdateSettings`: the method chosen because of merges does not overwrite the saved one. If
  // it cannot be saved, the push goes on.
  async function saveChoice({ method, remember }: RejectedAnswer, merges: boolean) {
    const settings = await api.settings();
    const updateMethod = merges ? settings.updateMethod : method;
    await api.saveSettings({ ...settings, updateMethod, pushAutoUpdate: remember });
  }

  // The rejection of `GitPushOperation.execute`, with Rust's `updatable`: it never forces.
  async function rejected(spec: PushSpec, result: PushResult, started: number) {
    const { branch, merges } = result.updatable as NonNullable<PushResult["updatable"]>;
    const settings = await api.settings().catch(() => null);
    if (!settings) {
      announce(spec, result, null, started);
      return;
    }
    let method = settings.updateMethod;
    let options: UpdateOptions = {};
    if (!settings.pushAutoUpdate) {
      const answer = await ui.ask<RejectedAnswer>({ kind: "pushRejected", branch, method, merges });
      if (!answer || generation !== started) {
        announce(spec, result, null, started);
        return;
      }
      await saveChoice(answer, merges).catch(() => {});
      method = answer.method;
      options = { rebaseMerges: true };
    }
    // It would update whichever repository is open now.
    if (generation !== started) {
      announce(spec, result, null, started);
      return;
    }
    const ran = await ui.updates.exclusive(() => retry(spec, method, options, 1, started));
    if (!ran) announce(spec, result, null, started);
  }

  // `started`: since when `spec` is valid. Each wait before pushing can end in another repository,
  // where the same remote and the same branch would be something else.
  async function run(spec: PushSpec, started = generation) {
    if (generation !== started) return;
    if (warning !== null) dismiss(warning);
    warning = null;
    const result = await attempt(PUSH_FAILED, () => api.push(spec));
    if (!result) return;
    if (result.updatable && generation === started) await rejected(spec, result, started);
    else announce(spec, result, null, started);
  }

  // `GitPushAfterCommitDialog.push`.
  async function keepPreviewForProtected() {
    const settings = await api.settings();
    if (settings.previewPushOnCommit) {
      await api.saveSettings({ ...settings, previewPushProtectedOnly: true });
    }
  }

  // The checkbox and the combo are a convenience: if they cannot be read or saved, the push goes on.
  async function open(
    branch: string | null,
    revision?: string,
    preview: boolean | null = null,
    known?: PushInfo,
    started = generation,
  ) {
    const info = known ?? (await attempt(LOAD_FAILED, () => api.info(branch)));
    if (!info) return;
    const tags = await api.tags().catch(() => null);
    if (generation !== started) return;
    const source = pushSource(info, revision);
    const { defineRemote } = ui;
    const choice = await ui.ask<PushChoice>({
      kind: "push",
      info,
      source,
      tags,
      preview,
      outgoing: (remote, target) => api.outgoing(source.revision, remote, target),
      defineRemote:
        defineRemote &&
        (async () =>
          (await defineRemote()) ? attempt(LOAD_FAILED, () => api.info(branch)) : null),
    });
    if (!choice || generation !== started) return;
    await api.saveTags(choice.tags).catch(() => {});
    const { remote, target, tags: chosen, noVerify, force } = choice;
    if (choice.rememberTarget) await api.rememberForceTarget(remote, target).catch(() => {});
    if (preview !== null && !choice.preview) await keepPreviewForProtected().catch(() => {});
    await run(
      {
        source: source.revision,
        remote,
        target,
        tags: chosen,
        force: force ? "lease" : null,
        noVerify,
      },
      started,
    );
  }

  // `GitPushAfterCommitDialog.showOrPush`. If the settings cannot be read, the dialog is shown.
  async function pushAfterCommit(started: number) {
    if (generation !== started) return;
    const info = await attempt(LOAD_FAILED, () => api.info(null));
    if (!info) return;
    const settings = await api.settings().catch(() => null);
    if (generation !== started) return;
    const { remote, branch: target } = info.target;
    const protectedTarget = remote !== null && isProtectedBranch(info.protectedBranches, target);
    const canPush = remote !== null && info.head !== null;
    if (remote === null || !settings || shouldPreview(settings, protectedTarget, canPush)) {
      await open(null, undefined, settings?.previewPushOnCommit ?? true, info, started);
      return;
    }
    const tags = await api.tags().catch(() => null);
    const { revision } = pushSource(info);
    await run({ source: revision, remote, target, tags, force: null, noVerify: false }, started);
  }

  // Before the commit: what it returns only pushes if this repository is still open at the end.
  function prepareCommitPush() {
    const started = generation;
    return () => pushAfterCommit(started);
  }

  function repositoryChanged() {
    generation += 1;
    if (warning !== null) dismiss(warning);
    warning = null;
  }

  // `Vcs.Push`: the current branch, or the commit of a detached HEAD.
  const push = () => open(null);

  // `GitPushBranchAction`.
  const pushBranch = (name: string) => open(name);

  // `GitPushUpToCommitAction`.
  async function pushUpTo(hash: string) {
    const found = await attempt(LOAD_FAILED, async () => ({ branch: await api.branchOf(hash) }));
    if (!found) return;
    if (found.branch === null) {
      notify({
        kind: "error",
        title: "Push failed to start",
        body: "'Push All up to Here' is supported for commit contained in a local branch only",
      });
      return;
    }
    await open(found.branch, hash);
  }

  return { push, pushBranch, pushUpTo, prepareCommitPush, repositoryChanged };
}
