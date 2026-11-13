import { Channel } from "@tauri-apps/api/core";
import * as api from "../lib/api";
import type {
  ChangeGroup,
  CommitOptions,
  CommitRequest,
  DeletedBranch,
  DeletedRemote,
  DeletedTag,
  LogFilter,
  LogRow,
  SelectedChanges,
  ViewOpened,
  ViewSpec,
  WorktreeSides,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { taskResult, trackTasks } from "../lib/tasks";

// Sends the commands through the real Tauri IPC, in WebKit and in release, against a disposable
// repository. It only enters the build with VITE_RAMIFY_SMOKE: see scripts/smoke_ipc.py. It changes the
// real repository, and therefore refuses to run outside a ramify-smoke folder.

const STEP_LIMIT_MS = 60_000;
const PREVIEW = 400;

export interface SmokeStep {
  command: string;
  ok: boolean;
  ipc: boolean;
  ms: number;
  detail: string;
}

export const smokeRepoAllowed = (path: string | null) => !!path && /\/ramify-smoke[^/]*\//.test(path);

// What Tauri or serde say when the two sides do not understand each other: the rest are errors of the
// operation, which here do not invalidate the command.
const IPC_FAILURE =
  /not found$|invalid args|missing (required key|field)|unknown (variant|field)|invalid type|invalid value/;

export const isIpcFailure = (failure: unknown) => IPC_FAILURE.test(textOf(failure));

const textOf = (failure: unknown) =>
  typeof failure === "string" ? failure : (errorText(failure) ?? String(failure));

const preview = (value: unknown) => (JSON.stringify(value) ?? "undefined").slice(0, PREVIEW);

const textOfSide = (sides: WorktreeSides | undefined) =>
  sides?.new.kind === "text" ? sides.new.content : "";

export async function runSmoke(): Promise<void> {
  const steps: SmokeStep[] = [];

  async function step<T>(command: string, run: () => Promise<T>): Promise<T | undefined> {
    const started = performance.now();
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error("no response")), STEP_LIMIT_MS),
    );
    const done = (ok: boolean, ipc: boolean, detail: string) =>
      steps.push({ command, ok, ipc, ms: Math.round(performance.now() - started), detail });
    try {
      const value = await Promise.race([run(), timeout]);
      done(true, false, preview(value));
      return value;
    } catch (failure) {
      done(false, isIpcFailure(failure), textOf(failure).slice(0, PREVIEW));
      return undefined;
    }
  }

  const task = <T>(command: string, start: () => Promise<number>) =>
    step(command, async () => taskResult<T>(await start()));

  await trackTasks();
  const repo = await step("initial_repo", api.initialRepo);
  if (!smokeRepoAllowed(repo ?? null)) {
    await api.perfReport(JSON.stringify({ refused: repo ?? null, steps }));
    return;
  }
  await step("app_version", api.appVersion);
  await step("perf_enabled", api.perfEnabled);
  await step("show_window", api.showWindow);
  await step("repo_open", () => api.repoOpen(repo!));

  await commitSteps();
  await logSteps();
  await branchSteps();
  await remoteSteps();

  await terminalSteps();
  await step("console_lines", api.consoleLines);
  await step("task_cancel", () => api.taskCancel(999_999));
  await step("askpass_answer", () => api.askpassAnswer(999_999, null));
  await step("hide_window", api.hideWindow);
  await api.perfReport(JSON.stringify({ steps }));

  async function commitSteps() {
    const status = await step("repo_status", () => api.repoStatus(false));
    await step("changelists_get", () => api.changelistsGet(status?.map((file) => file.path) ?? []));
    const first = await step("changelists_create", () => api.changelistsCreate("smoke"));
    const list = first?.lists.find((one) => one.name === "smoke")?.id ?? "";
    await step("changelists_rename", () => api.changelistsRename(list, "smoke 2"));
    await step("changelists_move", () => api.changelistsMove(["b.txt"], list));
    await step("changelists_set_active", () => api.changelistsSetActive(list));
    const hunks = await step("file_hunks", () => api.fileHunks("a.txt"));
    await step("changelists_move_hunks", () =>
      api.changelistsMoveHunks("a.txt", [hunks?.[0]?.id ?? ""], list),
    );
    await step("changelists_set_active", () =>
      api.changelistsSetActive(first?.lists.find((one) => one.id !== list)?.id ?? ""),
    );
    await step("changelists_remove", () => api.changelistsRemove(list));
    await step("changelists_import", api.changelistsImport);

    const sides = await step("worktree_sides", () => api.worktreeSides("a.txt"));
    const text = textOfSide(sides);
    await step("worktree_preview", () => api.worktreePreview("a.txt", `${text}smoke\n`));
    await step("file_write", () => api.fileWrite("a.txt", `${text}smoke\n`, sides?.hash ?? ""));
    await step("diff_window_open", () => api.diffWindowOpen("a.txt"));
    await new Promise((resolve) => setTimeout(resolve, 1_000));
    await step("diff_window_path", api.diffWindowPath);
    await step("diff_flushed", () => api.diffFlushed(false));

    const options: CommitOptions = {
      amend: false,
      signOff: false,
      skipHooks: false,
      author: null,
      cleanup: true,
    };
    const request = (message: string, added: string[]): CommitRequest => ({
      added,
      removed: [],
      partial: [],
      message,
      options,
    });
    const added = request("smoke: new", ["new.txt"]);
    await step("commit_warnings", () => api.commitWarnings(added, []));
    await step("commit_warnings_staged", api.commitWarningsStaged);
    await step("commit_fix_autocrlf", api.commitFixAutocrlf);
    await step("commit_changes", () => api.commitChanges(added));
    const left = await step("file_hunks", () => api.fileHunks("a.txt"));
    const selection = [{ hunk: left?.[0]?.id ?? "", lines: null }];
    await step("commit_changes_selected", () =>
      api.commitChangesSelected(request("smoke: lines", []), [{ path: "a.txt", selection }]),
    );

    const lines = await step("file_hunks", () => api.fileHunks("b.txt"));
    await step("lines_rollback", () =>
      api.linesRollback("b.txt", [{ hunk: lines?.[0]?.id ?? "", lines: null }]),
    );
    await step("rollback_files", () => api.rollbackFiles(["e.txt"], false));
    const copies = await step("snapshots_list", api.snapshotsList);
    await step("snapshots_restore", () => api.snapshotsRestore(copies?.[0]?.id ?? ""));
    await step("messages_list", api.messagesList);
    await step("head_message", api.headMessage);

    const local = await step("staging_sides", () => api.stagingSides("c.txt", "local"));
    await step("staging_preview", () => api.stagingPreview("c.txt", textOfSide(local)));
    await step("staging_stage_hunks", () =>
      api.stagingStageHunks("c.txt", [{ hunk: local?.hunks[0]?.id ?? "", lines: null }]),
    );
    const staged = await step("staging_sides", () => api.stagingSides("c.txt", "staged"));
    await step("staging_unstage_hunks", () =>
      api.stagingUnstageHunks("c.txt", [{ hunk: staged?.hunks[0]?.id ?? "", lines: null }]),
    );
    await step("staging_stage", () => api.stagingStage(["c.txt"]));
    await step("staging_unstage", () => api.stagingUnstage(["c.txt"]));
    await step("staging_revert", () => api.stagingRevert(["d.txt"]));
    await step("staging_stage", () => api.stagingStage(["c.txt"]));
    await step("commit_staged", () => api.commitStaged(options, "smoke: stage"));

    const settings = await step("settings_get", api.settingsGet);
    if (settings) await step("settings_save", () => api.settingsSave(settings));
    await step("git_version", () => api.gitVersion(null));

    const rest = (await step("repo_status", () => api.repoStatus(false))) ?? [];
    if (rest.length) {
      await step("rollback_files", () =>
        api.rollbackFiles(
          rest.map((file) => file.path),
          true,
        ),
      );
    }
  }

  async function logSteps() {
    const log = await step("log_open", api.logOpen);
    const viewId = log?.viewId ?? 0;
    const rows = (await step("log_rows", () => api.logRows(viewId, 0, 50))) ?? [];
    await step("log_view_set", () => api.logViewSet(viewId, { sort: "linear" }));
    await step("log_view_set", () => api.logViewSet(viewId, { sort: "standard" }));
    await step("log_graph_hover", () => api.logGraphHover(viewId, { kind: "node", row: 0 }));
    await step("log_graph_click", () => api.logGraphClick(viewId, { kind: "node", row: 0 }));
    await step("log_row_of", () => api.logRowOf(viewId, { kind: "ref", value: "v1" }));
    const merge = rows.find((row: LogRow) => row.print.edges.length > 2) ?? rows[0];
    const hash = merge?.hash ?? "";
    await step("log_find_rows", () => api.logFindRows(viewId, [hash]));
    await step("log_adjacent_rows", () => api.logAdjacentRows(viewId, 0, true));
    await step("log_current_branch", api.logCurrentBranch);

    const filter: LogFilter = {
      branches: ["main"],
      users: [],
      since: null,
      until: null,
      paths: ["a.txt"],
      text: { text: "smoke", regex: false, matchCase: false },
      hashes: [],
      revisions: [],
      ranges: [],
    };
    const spec: ViewSpec = { sort: "standard", longEdges: true, collapsed: false, filter };
    await task<ViewOpened>("log_view_open", () => api.logViewOpen(1, spec));
    await step("log_view_close", () => api.logViewClose(1));
    await step("log_users", api.logUsers);
    await step("log_refs", api.logRefs);
    await step("log_refs_changed", api.logRefsChanged);
    const options = await step("log_options_get", api.logOptionsGet);
    await step("log_options_set", () => api.logOptionsSet(options ?? {}));
    await step("commit_details", () => api.commitDetails([hash]));
    await task<api.Signature[]>("log_signatures", () => api.logSignatures([hash], 0));
    await step("branches_containing", () => api.branchesContaining(hash));
    const groups = await step<ChangeGroup[]>("commit_changes_of", () =>
      api.commitChangesOf(hash, "parents"),
    );
    await task<SelectedChanges>("commit_changes_start", () => api.commitChangesStart([hash], null));
    await step("log_recent_filters", api.logRecentFilters);
    await step("log_remember_filter", () => api.logRememberFilter("branch", ["main"]));
    const files = (await step("commit_files", () => api.commitFiles(hash))) ?? [];
    const file = files[0] ?? groups?.[0]?.files[0];
    if (file) await step("file_sides", () => api.fileSides(hash, file));
  }

  async function branchSteps() {
    await step("branch_list", api.branchList);
    await step("branch_current", api.branchCurrent);
    await step("branch_incoming_outgoing", api.branchIncomingOutgoing);
    await step("branch_refs_fingerprint", api.branchRefsFingerprint);
    await step("favorites_get", api.favoritesGet);
    await step("favorites_toggle", () => api.favoritesToggle("main", "local"));
    const options = await step("branch_options_get", api.branchOptionsGet);
    await step("branch_options_set", () => api.branchOptionsSet(options ?? {}));
    await step("pull_options_get", api.pullOptionsGet);
    await step("pull_options_set", () => api.pullOptionsSet(["ffOnly", "noVerify"]));
    await step("reset_mode_get", api.resetModeGet);
    await step("reset_mode_set", () => api.resetModeSet("keep"));

    await task("branch_checkout", () => api.branchCheckout("feature", "smart"));
    await task("branch_checkout", () => api.branchCheckout("main", "plain"));
    await task("fetch_all", api.fetchAll);
    await task("branch_checkout_remote", () =>
      api.branchCheckoutRemote("origin/extra", { name: null, dropCommits: false, mode: "plain" }),
    );
    await task("branch_checkout", () => api.branchCheckout("main", "plain"));
    await task("branch_create", () =>
      api.branchCreate({
        name: "smoke/new",
        start: null,
        checkout: false,
        overwrite: "no",
        mode: "plain",
      }),
    );
    await task("branch_rename", () => api.branchRename("smoke/new", "smoke/renamed"));
    const deleted = await task<DeletedBranch>("branch_delete", () =>
      api.branchDelete("smoke/renamed"),
    );
    if (deleted) await task("branch_restore", () => api.branchRestore(deleted));
    await step("branch_unmerged_commits", () => api.branchUnmergedCommits("main", "gone"));
    await step("branch_trackers", () => api.branchTrackers("origin/feature"));
    await task<DeletedRemote>("branch_delete_remote", () =>
      api.branchDeleteRemote("origin/tracked2", false),
    );
    await task("branch_rebase_onto", () => api.branchRebaseOnto("feature", "gone"));
    await step("repo_state", api.repoState);
    await step("git_menu_enable", () => api.gitMenuEnable([]));
    await task("operation_continue", api.operationContinue);
    await task("operation_skip", api.operationSkip);
    await task("operation_abort", () => api.operationAbort("rebasing", ""));
    await task("branch_checkout", () => api.branchCheckout("main", "plain"));
    await task("branch_reset", () => api.branchReset("HEAD", "mixed", "plain"));
    await task("tag_create", () => api.tagCreate("smoke-tag", "HEAD", "with message", false));
    const tag = await task<DeletedTag>("tag_delete", () => api.tagDelete("smoke-tag"));
    if (tag) await task("tag_restore", () => api.tagRestore(tag));
  }

  async function remoteSteps() {
    await task("fetch_remote", () => api.fetchRemote("origin"));
    await step("pull_info", api.pullInfo);
    await task("update_project", () => api.updateProject("merge", {}));
    await task("git_pull", () => api.gitPull("origin", "main", ["noFf"]));
    await task("update_branch", () => api.updateBranch("feature"));
    await task("update_abort", api.updateAbort);
    await task("update_restore", () =>
      api.updateRestore({ kind: "stash", sha: "0".repeat(40), message: "smoke", lists: [] }),
    );

    await step("push_info", () => api.pushInfo(null));
    const head = (await step("log_rows", async () => {
      const log = await api.logOpen();
      return api.logRows(log.viewId, 0, 1);
    }))?.[0]?.hash;
    await step("push_branch_of", () => api.pushBranchOf(head ?? ""));
    await task("push_outgoing", () => api.pushOutgoing("main", "origin", "main"));
    const spec = {
      source: "main",
      remote: "origin",
      target: "main",
      tags: "all" as const,
      force: null,
      noVerify: false,
    };
    await task("git_push", () => api.gitPush(spec));
    await task("git_push_update", () => api.gitPushUpdate(spec, "merge", {}, 0));
    await step("push_tags_get", api.pushTagsGet);
    await step("push_tags_set", () => api.pushTagsSet("currentBranch"));
    await step("repo_settings_get", api.repoSettingsGet);
    await step("protected_branches_set", () => api.protectedBranchesSet(["main", "master"], true));
    await step("github_links", () => api.githubLinks({ kind: "file", path: "a.txt" }));
    await step("github_generate_url", () => api.githubGenerateUrl("github.com"));
    // No account is listed in the smoke HOME: the refusal is the operation's, not the IPC's.
    await step("github_set_default", () => api.githubSetDefault("github.com/octocat"));
    await step("github_forget_connection", api.githubForgetConnection);
    await step("github_pr_search_saved", api.githubPrSearchSaved);
    // Without a connection it is not checked out.
    await step("github_pr_checked_out", () => api.githubPrCheckedOut(1));
    await step("github_pr_search_save", () =>
      api.githubPrSearchSave(
        { text: "smoke", state: "OPEN", assignee: null, review: null, author: null, label: null },
        true,
      ),
    );
    await step("force_push_target_remember", () => api.forcePushTargetRemember("origin", "main"));

    const remotes = await step("remote_list", api.remoteList);
    const url = remotes?.[0]?.url ?? "";
    await task("remote_add", () => api.remoteAdd("smoke", url));
    await task("remote_edit", () => api.remoteEdit("smoke", "smoke2", url));
    await task("remote_test_url", () => api.remoteTestUrl(url, null));
    await task("remote_remove", () => api.remoteRemove("smoke2"));
    // Unchecking only writes `commit.gpgSign false`: no gpg runs.
    await task("gpg_config_set", () => api.gpgConfigSet(null));
    await task("clone_repository", () => api.cloneRepository(url, `${repo}-clone`));
    await task("init_repository", () => api.initRepository(`${repo}-new`, false));
    await step("open_exclude_file", api.openExcludeFile);
  }

  // A real shell in the smoke repository: its output must come back through the channel as bytes.
  async function terminalSteps() {
    let seen = "";
    const decoder = new TextDecoder();
    const output = new Channel<ArrayBuffer | number[]>();
    output.onmessage = (bytes) => {
      seen += decoder.decode(bytes instanceof ArrayBuffer ? bytes : Uint8Array.from(bytes), { stream: true });
    };
    const shows = (text: string) =>
      new Promise<string>((resolve, reject) => {
        const started = Date.now();
        const poll = () => {
          if (seen.includes(text)) resolve(text);
          else if (Date.now() - started > 10_000) reject(new Error(`the terminal never showed ${text}`));
          else setTimeout(poll, 50);
        };
        poll();
      });
    const id = await step("terminal_open", () => api.terminalOpen(100, 30, output));
    if (id === undefined) return;
    await step("terminal_write", async () => {
      await api.terminalWrite(id, "echo smoke-$((6*7)) in $(basename \"$PWD\")\n");
      return shows("smoke-42 in ");
    });
    await step("terminal_resize", async () => {
      await api.terminalResize(id, 120, 40);
      await api.terminalWrite(id, "stty size\n");
      return shows("40 120");
    });
    await step("terminal_busy", async () => {
      await api.terminalWrite(id, "sleep 30\n");
      for (let tries = 0; tries < 100; tries++) {
        if (await api.terminalBusy(id)) return true;
        await new Promise((wait) => setTimeout(wait, 50));
      }
      throw new Error("sleep never took the foreground");
    });
    await step("terminal_close", async () => {
      const exited = new Promise<number>((resolve) => {
        void api.onTerminalExited(resolve);
      });
      await api.terminalClose(id);
      return Promise.race([
        exited,
        new Promise((_, reject) => setTimeout(() => reject(new Error("the session did not end")), 10_000)),
      ]);
    });
    await step("terminal_reset", api.terminalReset);
  }
}
