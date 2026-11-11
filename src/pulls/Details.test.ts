import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  CiJob,
  PullRequestChanges,
  PullRequestCommit,
  PullRequestDetails,
  PullRequestStatus,
} from "../lib/api";
import { taskResult } from "../lib/tasks";
import { loginFor } from "../hosting/accounts";
import { dismiss, notifications } from "../lib/notifications";
import { forgetToolWindows, registerToolWindow } from "../shell/toolWindows";
import { prettyDateTime } from "../log/draw";
import { account, connection } from "./fixtures";
import Details from "./Details.svelte";
import { ciSummary, statusLines } from "./details";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main" }),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emitTo: vi.fn() }));
vi.mock("../lib/tasks", () => ({ taskResult: vi.fn() }));
vi.mock("../hosting/accounts", async (original) => ({
  ...(await original<typeof import("../hosting/accounts")>()),
  loginFor: vi.fn(),
}));

// jsdom has no ResizeObserver, which the changes tree needs.
vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);

const BASE = "1".repeat(40);
const HEAD = "2".repeat(40);
const NEW_HEAD = "3".repeat(40);
const MERGE_BASE = "4".repeat(40);
const C1 = "a".repeat(40);
const C2 = "b".repeat(40);

function pullRequest(extra: Partial<PullRequestDetails> = {}): PullRequestDetails {
  return {
    number: 7,
    url: "https://github.com/o/r/pull/7",
    title: "Fix the crash",
    body: "It crashed.",
    state: "OPEN",
    isDraft: false,
    baseRefName: "main",
    baseRefOid: BASE,
    headRefName: "fix",
    headRefOid: HEAD,
    headRepository: {
      owner: "o",
      isFork: false,
      nameWithOwner: "o/r",
      url: "https://github.com/o/r",
      sshUrl: "git@github.com:o/r.git",
    },
    reviewers: [{ name: "Alice", state: "ACCEPTED" }],
    ...extra,
  };
}

function commit(oid: string, headline: string, parents: string[]): PullRequestCommit {
  return {
    oid,
    abbreviatedOid: oid.slice(0, 7),
    headline,
    author: "Alice",
    committedDate: new Date(2020, 0, 2, 10, 0, 0).toISOString(),
    parents,
    url: `https://github.com/o/r/commit/${oid}`,
  };
}

const status = (extra: Partial<PullRequestStatus> = {}): PullRequestStatus => ({
  headRefOid: HEAD,
  jobs: [],
  hasConflicts: false,
  requiredApprovals: 0,
  isRestricted: false,
  ...extra,
});

const job = (name: string, state: CiJob["state"], isRequired = false, url: string | null = null): CiJob => ({
  name,
  state,
  isRequired,
  url,
});

const changed = (path: string, before = MERGE_BASE, after = HEAD, kind = "M"): PullRequestChanges["changes"][number] => ({
  path,
  oldPath: null,
  status: kind,
  before,
  after,
});

type Answer = unknown;
// What each task command answers, in order: a value, an error (`{ kind, message }`), or a
// promise that the test resolves.
let queues: Record<string, Answer[]>;
// What `github_pr_checked_out` answers, in order; the last one stays.
let checkedOut: boolean[];
let ids = 0;
const answers = new Map<number, Answer>();

const calls = (command: string) =>
  vi.mocked(invoke).mock.calls.filter(([name]) => name === command).map(([, args]) => args as Record<string, unknown>);

// Every answer already given has been read.
const settle = () => new Promise((done) => setTimeout(done, 20));

function held<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve: (value: T) => void = () => {};
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

beforeEach(() => {
  forgetToolWindows();
  ids = 0;
  answers.clear();
  checkedOut = [false];
  queues = {
    github_pr_details: [],
    github_pr_status: [],
    github_pr_commits: [],
    github_pr_changes: [],
  };
  vi.mocked(invoke).mockReset();
  vi.mocked(taskResult).mockReset();
  vi.mocked(loginFor).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command in queues) {
      const id = ++ids;
      const queue = queues[command];
      answers.set(id, queue.length > 1 ? queue.shift() : queue[0]);
      return id;
    }
    switch (command) {
      case "github_pr_checked_out":
        return checkedOut.length > 1 ? checkedOut.shift() : checkedOut[0];
      case "task_cancel":
      case "open_url":
      case "clipboard_write":
      case "diff_window_open_request":
        return undefined;
      default:
        throw new Error(`unexpected command: ${command}`);
    }
  });
  vi.mocked(taskResult).mockImplementation(async (id) => {
    const answer = await answers.get(id);
    if (answer && typeof answer === "object" && "kind" in answer && "message" in answer) throw answer;
    return answer;
  });
});

afterEach(() => {
  cleanup();
  for (const shown of get(notifications)) dismiss(shown.id);
});

function answer(details: Answer, rest: { status?: Answer; commits?: Answer; changes?: Answer[] } = {}) {
  queues.github_pr_details = [details];
  queues.github_pr_status = [rest.status ?? status()];
  queues.github_pr_commits = [rest.commits ?? [commit(C1, "First", [MERGE_BASE]), commit(HEAD, "Second", [C1])]];
  queues.github_pr_changes = rest.changes ?? [{ changes: [changed("src/main.rs")], body: null }];
}

function show(number = 7) {
  return render(Details, { connection: connection("origin", "github.com/octocat"), account: account("octocat"), number });
}

const row = (name: string) => screen.getByRole("treeitem", { name });
const statuses = () => screen.getByLabelText("Status");

async function menuItems(open: () => Promise<unknown>) {
  await open();
  const menu = await screen.findByRole("menu");
  return { menu, labels: within(menu).getAllByRole("menuitem").map((item) => item.getAttribute("aria-label")) };
}

test("the_title_row_has_the_number_as_a_link_and_the_state_tag", async () => {
  answer(pullRequest({ isDraft: true, state: "OPEN" }));
  show();

  const link = await screen.findByRole("button", { name: "#7" });
  expect(link.title).toBe("Open on GitHub");
  expect(screen.getByText("DRAFT")).toBeTruthy();
  await fireEvent.click(link);
  expect(calls("open_url")).toEqual([{ url: "https://github.com/o/r/pull/7" }]);
  // The branch: `owner:branch` only for a fork or a branch named as the base.
  expect(screen.getByLabelText("Branch").textContent?.trim()).toBe("fix");
});

test("a_fork_shows_its_owner_before_the_branch", async () => {
  const fork = pullRequest().headRepository!;
  answer(pullRequest({ headRepository: { ...fork, owner: "forker", isFork: true } }));
  show();

  await waitFor(() => expect(screen.getByLabelText("Branch").textContent?.trim()).toBe("forker:fix"));
});

// The check-URL decision (2026-11-09): a job's page on the connection's server opens at once; an
// `https://` page elsewhere (an external CI) opens after a confirmation that shows the full
// address; anything else (`http://` elsewhere, another scheme) stays disabled with Ramify's own
// tooltip, `refusedUrlText`.
const EXTERNAL = "https://ci.example.com/o/r/42?token=abc&next=<img src=x onerror=alert(1)>#frag";

// What the dialog shows and what opens: the URL as `new URL` normalises it, once.
const SHOWN = new URL(EXTERNAL).href;

async function openDetailsMenu(jobs: CiJob[]) {
  answer(pullRequest(), { status: status({ jobs }) });
  show();
  const statusBox = await screen.findByLabelText("Status");
  await fireEvent.click(await within(statusBox).findByRole("button", { name: "Details…" }));
  const menu = await screen.findByRole("menu", { name: "Open in Browser" });
  return (name: string) => within(menu).getByRole("menuitem", { name }) as HTMLButtonElement;
}

test("an_external_https_job_is_enabled_and_asks_before_opening", async () => {
  const row = await openDetailsMenu([job("circleci", "FAILED", false, EXTERNAL)]);

  expect([row("circleci").disabled, row("circleci").title]).toEqual([false, "Failed"]);
  await fireEvent.click(row("circleci"));

  const dialog = await screen.findByRole("dialog", { name: "Open External Link" });
  expect(dialog.textContent).toContain("This link leaves GitHub:");
  expect(within(dialog).getByText(SHOWN)).toBeTruthy();
  expect(calls("open_url")).toEqual([]);

  await fireEvent.click(within(dialog).getByRole("button", { name: "Open" }));
  expect(calls("open_url")).toEqual([{ url: SHOWN }]);
  expect(screen.queryByRole("dialog", { name: "Open External Link" })).toBeNull();
});

test("cancel_and_escape_on_the_external_link_dialog_open_nothing", async () => {
  const row = await openDetailsMenu([job("circleci", "FAILED", false, EXTERNAL)]);
  await fireEvent.click(row("circleci"));
  const dialog = await screen.findByRole("dialog", { name: "Open External Link" });
  await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("dialog", { name: "Open External Link" })).toBeNull();
  expect(calls("open_url")).toEqual([]);

  await fireEvent.click(within(statuses()).getByRole("button", { name: "Details…" }));
  await fireEvent.click(await screen.findByRole("menuitem", { name: "circleci" }));
  const again = await screen.findByRole("dialog", { name: "Open External Link" });
  await fireEvent.keyDown(again, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "Open External Link" })).toBeNull();
  expect(calls("open_url")).toEqual([]);
});

test("an_external_link_shows_and_opens_the_normalised_address", async () => {
  const lookalike = "https://g\u0456thub.com/x y";
  const row = await openDetailsMenu([job("ci", "FAILED", false, lookalike)]);
  await fireEvent.click(row("ci"));
  const dialog = await screen.findByRole("dialog", { name: "Open External Link" });
  const shown = new URL(lookalike).href;
  expect(shown).toMatch(/^https:\/\/xn--/);
  expect(shown).toContain("%20");
  expect(within(dialog).getByText(shown)).toBeTruthy();
  await fireEvent.click(within(dialog).getByRole("button", { name: "Open" }));
  expect(calls("open_url")).toEqual([{ url: shown }]);
});

test("an_external_link_is_shown_as_text", async () => {
  const row = await openDetailsMenu([job("circleci", "FAILED", false, EXTERNAL)]);
  await fireEvent.click(row("circleci"));
  const dialog = await screen.findByRole("dialog", { name: "Open External Link" });
  expect(within(dialog).getByText(SHOWN).textContent).toBe(SHOWN);
  expect(dialog.querySelector("img")).toBeNull();
});

test("an_external_http_job_and_a_javascript_job_stay_disabled", async () => {
  const row = await openDetailsMenu([
    job("evil", "FAILED", true, "javascript:alert(1)"),
    job("elsewhere", "SUCCESS", false, "http://ci.example.com/o/r/2"),
    job("file", "SUCCESS", false, "file:///etc/passwd"),
    job("same-http", "SUCCESS", false, "http://github.com/o/r/runs/2"),
  ]);
  const refused = "Only an https:// address on github.com can be opened";
  expect(
    [row("evil"), row("elsewhere"), row("file"), row("same-http")].map((item) => [item.disabled, item.title]),
  ).toEqual([
    [true, `Failed\n${refused}`],
    [true, `Success\n${refused}`],
    [true, `Success\n${refused}`],
    [true, `Success\n${refused}`],
  ]);
  await fireEvent.click(row("evil"));
  await fireEvent.click(row("elsewhere"));
  expect(screen.queryByRole("dialog", { name: "Open External Link" })).toBeNull();
  expect(calls("open_url")).toEqual([]);
  expect(get(notifications)).toEqual([]);
});

test("an_external_link_that_would_show_differently_from_what_opens_stays_disabled", async () => {
  const bad: [string, string][] = [
    ["rlo", "https://ci.example.com/a\u202Eb"],
    ["zwsp", "https://ci.example.com/a\u200Bb"],
    ["isolate", "https://ci.example.com/a\u2066b"],
    ["bom", "https://ci.example.com/a\uFEFFb"],
    ["c0", "https://ci.example.com/a\u0007b"],
    ["c1", "https://ci.example.com/a\u0085b"],
    ["tab", "https://ci.example.com/a\tb"],
    ["user-pass", "https://user:pass@ci.example.com/o/r/1"],
    ["user", "https://user@ci.example.com/o/r/1"],
  ];
  const row = await openDetailsMenu(bad.map(([name, url]) => job(name, "FAILED", false, url)));
  const refused = "Only an https:// address on github.com can be opened";
  for (const [name] of bad) {
    expect([name, row(name).disabled, row(name).title]).toEqual([name, true, `Failed\n${refused}`]);
    await fireEvent.click(row(name));
  }
  expect(screen.queryByRole("dialog", { name: "Open External Link" })).toBeNull();
  expect(calls("open_url")).toEqual([]);
});

test("the_description_is_plain_text_and_a_same_host_job_opens_at_once", async () => {
  answer(
    pullRequest({ title: "<img src=x onerror=alert(1)>", body: "<b>bold</b>\n‮right to left" }),
    { status: status({ jobs: [job("build", "SUCCESS", true, "https://github.com/o/r/runs/1")] }) },
  );
  const { container } = show();

  expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeTruthy();
  const description = screen.getByLabelText("Description");
  expect(description.textContent).toContain("<b>bold</b>");
  expect(container.querySelector("img, b")).toBeNull();

  await fireEvent.click(await within(statuses()).findByRole("button", { name: "Details…" }));
  const build = await screen.findByRole("menuitem", { name: "build" });
  expect([(build as HTMLButtonElement).disabled, build.title]).toEqual([false, "Success"]);
  await fireEvent.click(build);
  expect(screen.queryByRole("dialog", { name: "Open External Link" })).toBeNull();
  expect(calls("open_url")).toEqual([{ url: "https://github.com/o/r/runs/1" }]);
});

test("an_empty_body_says_no_description_provided", async () => {
  answer(pullRequest({ body: "" }));
  show();

  const empty = await screen.findByText("No description provided");
  expect(empty.classList.contains("empty")).toBe(true);
});

test("the_ci_line_follows_241", async () => {
  const text = (jobs: CiJob[]) => ciSummary(jobs)?.text ?? null;
  // No jobs: no line.
  expect(text([])).toBeNull();
  // 1. Every required job passed, also when none is required, whatever the others did.
  expect(text([job("a", "FAILED"), job("b", "PENDING")])).toBe("All required checks have passed");
  expect(text([job("a", "SUCCESS", true), job("b", "FAILED")])).toBe("All required checks have passed");
  // 2. Some pending and some failed.
  expect(text([job("a", "PENDING", true), job("b", "FAILED")])).toBe("Checks in progress, and some already failed…");
  // 3. Some pending.
  expect(text([job("a", "PENDING", true), job("b", "SUCCESS")])).toBe("Checks in progress…");
  // 4. Otherwise.
  expect(text([job("a", "FAILED", true)])).toBe("Checks failed");
  expect(text([job("a", "SKIPPED", true)])).toBe("Checks failed");

  answer(pullRequest(), { status: status({ jobs: [job("a", "FAILED", true), job("b", "PENDING")] }) });
  show();
  await waitFor(() => expect(statuses().textContent).toContain("Checks in progress, and some already failed…"));
  expect(within(statuses()).getByRole("img", { name: "Failed, in progress" })).toBeTruthy();
});

test("required_approvals_show_only_when_blocked_and_not_failing", () => {
  const texts = (details: PullRequestDetails, shown: PullRequestStatus) =>
    statusLines(details, shown)!.map((line) => line.text);
  const required = "At least 2 approving review is required by reviewers with write access";

  // Rust gives a count only when GitHub blocks on the reviews alone; a draft shows none.
  expect(texts(pullRequest(), status({ requiredApprovals: 2 }))).toContain(required);
  expect(texts(pullRequest({ isDraft: true }), status({ requiredApprovals: 2 }))).not.toContain(required);
  expect(texts(pullRequest(), status({ requiredApprovals: 0 }))).not.toContain(required);
  // The order of 241, without the access lines.
  expect(
    texts(
      pullRequest({ reviewers: [] }),
      status({ jobs: [job("a", "FAILED", true)], hasConflicts: true, requiredApprovals: 2, isRestricted: true }),
    ),
  ).toEqual([
    "Checks failed",
    "Branch has conflicts that must be resolved",
    required,
    "You are not authorized to merge request into this branch",
    "Missing reviewer",
  ]);
  expect(
    texts(
      pullRequest({
        isDraft: true,
        reviewers: [
          { name: "Alice", state: "ACCEPTED" },
          { name: "Bob", state: "WAIT_FOR_UPDATES" },
          { name: "team", state: "NEED_REVIEW" },
        ],
      }),
      status({ hasConflicts: null, isRestricted: true }),
    ),
  ).toEqual(["Alice approved review", "Bob is waiting for updates", "Waiting for review from team"]);
  // Until the status loads, only its loading line.
  expect(statusLines(pullRequest(), null)).toBeNull();
});

test("the_status_says_loading_until_it_loads_and_after_an_error", async () => {
  answer(pullRequest(), { status: { kind: "failed", message: "502 Bad Gateway" } });
  show();

  await screen.findByRole("treeitem", { name: "main.rs" });
  expect(statuses().textContent?.trim()).toBe("Loading pull request status");
});

test("the_diff_title_is_diff_for_pull_request_n", async () => {
  answer(pullRequest({ number: 1234 }), {
    changes: [{ changes: [changed("src/main.rs"), changed("new.txt", MERGE_BASE, HEAD, "A")], body: null }],
  });
  show(1234);

  const file = await screen.findByRole("treeitem", { name: "main.rs" });
  await fireEvent.click(file);
  await fireEvent.dblClick(file);

  expect(calls("diff_window_open_request")).toEqual([
    {
      request: {
        kind: "revisions",
        title: "Diff for Pull Request #1234",
        files: [
          {
            left: { revision: MERGE_BASE, path: "src/main.rs", title: MERGE_BASE },
            right: { revision: HEAD, path: "src/main.rs", title: HEAD },
          },
        ],
      },
    },
  ]);
  // ⌘D in the tree is `Show Diff` too.
  await fireEvent.keyDown(file, { key: "d", metaKey: true });
  expect(calls("diff_window_open_request")).toHaveLength(2);
  const { labels } = await menuItems(() => fireEvent.contextMenu(row("new.txt")));
  expect(labels).toEqual([
    "Show Diff",
    "Jump to Source",
    "View Options",
    "Expand All",
    "Collapse All",
    "Refresh Pull Request",
    "Open Pull Request in Browser",
    "Copy Pull Request URL",
  ]);
});

test("the_changes_wait_for_the_details_and_ask_for_their_base_and_head", async () => {
  answer(pullRequest());
  show();

  await screen.findByRole("treeitem", { name: "main.rs" });
  expect(calls("github_pr_changes")).toEqual([
    { number: 7, baseRef: "main", baseOid: BASE, headOid: HEAD, selection: null },
  ]);
});

test("an_empty_pull_request_says_it_has_no_changes", async () => {
  answer(pullRequest(), { changes: [{ changes: [], body: null }] });
  show();

  expect(await screen.findByText("Pull request does not contain any changes")).toBeTruthy();
});

test("a_failed_fetch_shows_cant_load_changes_and_retry_fetches_again", async () => {
  answer(pullRequest(), {
    changes: [
      { kind: "failed", message: "fatal: couldn't find remote ref refs/pull/7/head" },
      { changes: [changed("src/main.rs")], body: null },
    ],
  });
  show();

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Can't load changes");
  expect(alert.textContent).toContain("fatal: couldn't find remote ref refs/pull/7/head");
  // The rest of the tab is there.
  expect(screen.getByText("Fix the crash")).toBeTruthy();

  await fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));

  expect(await screen.findByRole("treeitem", { name: "main.rs" })).toBeTruthy();
  expect(calls("github_pr_changes")).toHaveLength(2);
  expect(calls("github_pr_changes")[1]).toEqual({
    number: 7,
    baseRef: "main",
    baseOid: BASE,
    headOid: HEAD,
    selection: null,
  });
});

test("cant_load_details_offers_retry_or_re_login", async () => {
  answer({ kind: "failed", message: "Server is unreachable" });
  show();

  const alert = await screen.findByRole("alert");
  expect(alert.textContent).toContain("Can't load details");
  expect(alert.textContent).toContain("Server is unreachable");
  queues.github_pr_details = [{ kind: "authentication", message: "Request response: Bad credentials." }];
  await fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));

  const again = await screen.findByRole("alert");
  await waitFor(() => expect(again.textContent).toContain("Bad credentials"));
  vi.mocked(loginFor).mockResolvedValueOnce(account("octocat"));
  queues.github_pr_details = [pullRequest()];
  await fireEvent.click(within(again).getByRole("button", { name: "Re-Login" }));

  expect(vi.mocked(loginFor).mock.calls).toEqual([["github.com", "github.com/octocat"]]);
  expect(await screen.findByText("Fix the crash")).toBeTruthy();
  expect(calls("github_pr_details")).toHaveLength(3);
});

test("the_commits_dropdown_chooses_one_commit_or_all_of_them", async () => {
  answer(pullRequest(), {
    commits: [commit(C1, "First", [MERGE_BASE]), commit(C2, "Second", [C1]), commit(HEAD, "Third", [C2])],
    changes: [
      { changes: [changed("src/main.rs")], body: null },
      { changes: [changed("second.txt", C1, C2)], body: "The body\nof the second." },
      { changes: [changed("third.txt", C2, HEAD)], body: "" },
      { changes: [changed("src/main.rs")], body: null },
    ],
  });
  show();

  const link = await screen.findByRole("button", { name: "3 commits" });
  expect(screen.getByText("Changes from")).toBeTruthy();
  await fireEvent.click(link);
  const list = await screen.findByRole("listbox", { name: "Commits" });
  const options = within(list).getAllByRole("option");
  const date = prettyDateTime(new Date(2020, 0, 2, 10, 0, 0).getTime(), Date.now());
  expect(options.map((option) => option.textContent?.replace(/\s+/g, " ").trim())).toEqual([
    "✓ All commits (3)",
    `First Alice, ${date}`,
    `Second Alice, ${date}`,
    `Third Alice, ${date}`,
  ]);
  const search = screen.getByPlaceholderText("Search commits");
  await fireEvent.input(search, { target: { value: "sec" } });
  expect(within(list).getAllByRole("option").map((option) => option.textContent)).toHaveLength(1);
  await fireEvent.click(within(list).getByRole("option"));

  expect(await screen.findByRole("treeitem", { name: "second.txt" })).toBeTruthy();
  expect(calls("github_pr_changes")[1]).toMatchObject({ selection: 1, headOid: HEAD });
  expect(screen.getByRole("button", { name: C2.slice(0, 7) })).toBeTruthy();
  // The chosen commit, with its body from git behind `Show details`.
  const info = screen.getByLabelText("Commit");
  expect(info.textContent).toContain("Second");
  expect(info.textContent).not.toContain("of the second");
  await fireEvent.click(within(info).getByRole("button", { name: "Show details" }));
  expect(info.textContent).toContain("The body\nof the second.");
  expect(info.textContent).toContain(`Alice, ${date}`);
  expect(within(info).getByRole("button", { name: "Hide details" })).toBeTruthy();

  // Next and previous.
  await fireEvent.click(screen.getByRole("button", { name: "Next commit" }));
  expect(await screen.findByRole("treeitem", { name: "third.txt" })).toBeTruthy();
  expect((screen.getByRole("button", { name: "Next commit" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Previous commit" }) as HTMLButtonElement).disabled).toBe(false);

  // A chooser closed without a choice is all commits, as in 241.
  await fireEvent.click(screen.getByRole("button", { name: HEAD.slice(0, 7) }));
  await fireEvent.keyDown(screen.getByPlaceholderText("Search commits"), { key: "Escape" });
  expect(await screen.findByRole("button", { name: "3 commits" })).toBeTruthy();
  expect(calls("github_pr_changes")[3]).toMatchObject({ selection: null });
  expect(screen.queryByLabelText("Commit")).toBeNull();
});

test("one_commit_is_changes_and_the_dropdown_is_off", async () => {
  answer(pullRequest(), { commits: [commit(HEAD, "Only", [MERGE_BASE])] });
  show();

  const link = await screen.findByRole("button", { name: "1 commits" });
  expect((link as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText("Changes")).toBeTruthy();
});

test("commits_for_another_head_are_dropped", async () => {
  answer(pullRequest(), { commits: [commit(C1, "Old", [MERGE_BASE]), commit(NEW_HEAD, "New", [C1])] });
  show();

  await screen.findByRole("treeitem", { name: "main.rs" });
  await settle();
  expect(screen.getByRole("button", { name: "0 commits" })).toBeTruthy();
});

test("an_answer_for_an_old_head_or_another_repository_is_dropped", async () => {
  // A refresh while the changes of the old head still load: their answer is dropped.
  const old = held<PullRequestChanges>();
  answer(pullRequest(), {
    status: status({ headRefOid: NEW_HEAD, jobs: [job("a", "SUCCESS")] }),
    changes: [old.promise, { changes: [changed("new.txt", MERGE_BASE, NEW_HEAD)], body: null }],
  });
  show();
  await screen.findByText("Fix the crash");
  // A status built for another head stays unread.
  await waitFor(() => expect(calls("github_pr_status")).toHaveLength(1));
  await settle();
  expect(statuses().textContent?.trim()).toBe("Loading pull request status");

  queues.github_pr_details = [pullRequest({ headRefOid: NEW_HEAD })];
  await fireEvent.contextMenu(screen.getByText("Fix the crash"));
  await fireEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Refresh Pull Request" }));
  expect(await screen.findByRole("treeitem", { name: "new.txt" })).toBeTruthy();
  old.resolve({ changes: [changed("old.txt")], body: null });
  await settle();
  expect(screen.queryByRole("treeitem", { name: "old.txt" })).toBeNull();
  expect(screen.getByRole("treeitem", { name: "new.txt" })).toBeTruthy();
  cleanup();

  // Another repository opens while the details load: their answer is dropped.
  registerToolWindow("pulls");
  const details = held<PullRequestDetails>();
  answer(details.promise);
  show();
  await waitFor(() => expect(calls("github_pr_details")).toHaveLength(3));
  // The task is running: its answer, not its start, comes after the switch.
  await settle();
  forgetToolWindows();
  details.resolve(pullRequest());
  await settle();
  expect(screen.queryByText("Fix the crash")).toBeNull();
  expect(screen.getByRole("status").textContent).toBe("Loading…");
});

test("refresh_reloads_details_status_and_changes_for_the_new_head", async () => {
  answer(pullRequest(), {
    changes: [
      { changes: [changed("src/main.rs")], body: null },
      { changes: [changed("pushed.txt", MERGE_BASE, NEW_HEAD)], body: null },
    ],
  });
  show();
  await screen.findByRole("treeitem", { name: "main.rs" });

  queues.github_pr_details = [pullRequest({ headRefOid: NEW_HEAD, title: "Fix the crash again" })];
  queues.github_pr_status = [status({ headRefOid: NEW_HEAD, hasConflicts: true })];
  queues.github_pr_commits = [[commit(C1, "First", [MERGE_BASE]), commit(NEW_HEAD, "Pushed", [C1])]];
  const { menu, labels } = await menuItems(() => fireEvent.contextMenu(screen.getByText("Fix the crash")));
  expect(labels).toEqual(["Refresh Pull Request", "Open Pull Request in Browser", "Copy Pull Request URL"]);
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Refresh Pull Request" }));

  expect(await screen.findByRole("treeitem", { name: "pushed.txt" })).toBeTruthy();
  expect(screen.getByText("Fix the crash again")).toBeTruthy();
  await waitFor(() => expect(statuses().textContent).toContain("Branch has conflicts that must be resolved"));
  expect(calls("github_pr_details")).toHaveLength(2);
  expect(calls("github_pr_status")).toHaveLength(2);
  expect(calls("github_pr_commits")).toHaveLength(2);
  expect(calls("github_pr_changes")).toEqual([
    { number: 7, baseRef: "main", baseOid: BASE, headOid: HEAD, selection: null },
    { number: 7, baseRef: "main", baseOid: BASE, headOid: NEW_HEAD, selection: null },
  ]);
});

test("copy_pull_request_url_copies_githubs_url", async () => {
  answer(pullRequest());
  show();

  const { menu } = await menuItems(async () => fireEvent.contextMenu(await screen.findByText("Fix the crash")));
  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Copy Pull Request URL" }));

  expect(calls("clipboard_write")).toEqual([{ text: "https://github.com/o/r/pull/7" }]);
});

test("the_branch_label_has_the_checkout_chooser_and_the_checked_out_icon", async () => {
  let repoChanged: (() => void) | undefined;
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    if (name === "repo-changed") repoChanged = () => (handler as (event: unknown) => void)({ payload: null });
    return () => {};
  });
  checkedOut = [false, true];
  queues.github_pr_checkout = [{ kind: "gone" }];
  answer(
    pullRequest({
      headRepository: {
        owner: "forker",
        isFork: true,
        nameWithOwner: "forker/r",
        url: "https://github.com/forker/r",
        sshUrl: "git@github.com:forker/r.git",
      },
    }),
  );
  show();

  const label = await screen.findByLabelText("Branch");
  await waitFor(() => expect(calls("github_pr_checked_out")).toEqual([{ number: 7 }]));
  expect(label.dataset.checkedOut).toBe("false");
  // A checkout from the terminal: the icon follows `repo-changed`.
  repoChanged?.();
  await waitFor(() => expect(label.dataset.checkedOut).toBe("true"));

  await fireEvent.click(within(label).getByRole("button", { name: "forker:fix" }));
  const menu = await screen.findByRole("menu", { name: "Branch" });
  const labels = within(menu)
    .getAllByRole("menuitem")
    .map((item) => item.getAttribute("aria-label"));
  expect(labels).toEqual(["Checkout 'forker:fix'…", "Show 'forker:fix' in Git Log"]);
  expect(menu.textContent).toContain("main ← forker:fix");

  await fireEvent.click(within(menu).getByRole("menuitem", { name: "Checkout 'forker:fix'…" }));
  await waitFor(() => expect(calls("github_pr_checkout")).toEqual([{ number: 7 }]));
});
