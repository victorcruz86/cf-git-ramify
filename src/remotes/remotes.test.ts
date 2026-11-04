import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, expect, test, vi } from "vitest";
import type { Remote } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import CloneDialog from "./CloneDialog.svelte";
import DefineRemoteDialog from "./DefineRemoteDialog.svelte";
import InitDialog from "./InitDialog.svelte";
import {
  addRemote,
  cloneFlow,
  editRemote,
  initFlow,
  openExclude,
  remoteProblem,
  removeMessage,
  removeRemote,
  type RemotesApi,
} from "./model";
import RemotesDialog from "./RemotesDialog.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }));

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
});

const ORIGIN: Remote = { name: "origin", url: "https://example.com/a.git", pushUrl: null };
const COPY: Remote = { name: "copy", url: "/srv/copy.git", pushUrl: null };

function api(overrides: Partial<RemotesApi> = {}): RemotesApi {
  return {
    list: vi.fn(async () => [ORIGIN, COPY]),
    add: vi.fn(async () => {}),
    edit: vi.fn(async () => {}),
    remove: vi.fn(async () => {}),
    testUrl: vi.fn(async () => {}),
    clone: vi.fn(async () => ({ path: "/work/copy", warning: null })),
    init: vi.fn(async (path: string) => ({ kind: "created" as const, path })),
    openExclude: vi.fn(async () => true),
    openInNewWindow: vi.fn(async () => {}),
    ...overrides,
  };
}

const shown = () => get(notifications);

test("literal_remote_errors", () => {
  expect(remoteProblem("", "https://x", [])).toEqual({ name: "Remote name cannot be empty" });
  expect(remoteProblem("  ", "https://x", [])).toEqual({ name: "Remote name cannot be empty" });
  expect(remoteProblem("origin", "https://x", ["origin"])).toEqual({
    name: "Remote name 'origin' is already used",
  });
  expect(remoteProblem("copy", "", ["origin"])).toEqual({ url: "Remote URL cannot be empty" });
  expect(remoteProblem("copy", "https://x", ["origin"])).toEqual({});
  expect(remoteProblem("bad name", "https://x", [])).toEqual({
    name: "Remote name contains illegal characters",
  });
});

test("the_name_validator_is_the_git_ref_one", () => {
  for (const bad of ["-x", ".x", "/x", "a..b", "a//b", "a~b", "a^b", "a:b", "a?b", "a*b", "a[b", "a\\b", "a@{b", "x.", "x/", "x.lock", "a\tb"]) {
    expect(remoteProblem(bad, "u", []).name, bad).toBe("Remote name contains illegal characters");
  }
  for (const good of ["origin", "my-remote", "a/b", "v1.2", "UPSTREAM"]) {
    expect(remoteProblem(good, "u", []).name, good).toBeUndefined();
  }
});

test("the_define_dialog_shows_the_literal_errors", async () => {
  render(DefineRemoteDialog, {
    initialName: "",
    initialUrl: "",
    taken: ["origin"],
    test: vi.fn(async () => {}),
    onanswer: vi.fn(),
  });
  const ok = screen.getByRole("button", { name: "OK" });

  await fireEvent.click(ok);
  expect(screen.getByText("Remote name cannot be empty")).toBeTruthy();

  await fireEvent.input(screen.getByLabelText("Name:"), { target: { value: "origin" } });
  await fireEvent.click(ok);
  expect(screen.getByText("Remote name 'origin' is already used")).toBeTruthy();

  await fireEvent.input(screen.getByLabelText("Name:"), { target: { value: "copy" } });
  await fireEvent.click(ok);
  expect(screen.getByText("Remote URL cannot be empty")).toBeTruthy();
});

test("define_tests_the_url_and_only_then_answers", async () => {
  const onanswer = vi.fn();
  const test_ = vi.fn(async () => {});
  render(DefineRemoteDialog, {
    initialName: "origin",
    initialUrl: "",
    taken: [],
    test: test_,
    onanswer,
  });

  await fireEvent.input(screen.getByLabelText("URL:"), { target: { value: " /srv/a.git " } });
  await fireEvent.click(screen.getByRole("button", { name: "OK" }));

  await waitFor(() => expect(onanswer).toHaveBeenCalledWith({ name: "origin", url: "/srv/a.git" }));
  expect(test_).toHaveBeenCalledWith("/srv/a.git", "origin");
});

test("a_url_that_does_not_answer_stays_in_the_dialog_with_the_git_error", async () => {
  const onanswer = vi.fn();
  const failing = vi.fn(async () => {
    throw { kind: "failed", message: "fatal: '/x' does not appear to be a git repository" };
  });
  render(DefineRemoteDialog, {
    initialName: "origin",
    initialUrl: "/x",
    taken: [],
    test: failing,
    onanswer,
  });

  await fireEvent.click(screen.getByRole("button", { name: "OK" }));

  expect(await screen.findByText(/does not appear to be a git repository/)).toBeTruthy();
  expect(onanswer).not.toHaveBeenCalled();
  expect((screen.getByLabelText("URL:") as HTMLInputElement).disabled).toBe(false);
});

test("editing_allows_keeping_its_own_name", () => {
  expect(remoteProblem("origin", "u", ["copy"], "origin")).toEqual({});
  expect(remoteProblem("copy", "u", ["copy"], "origin")).toEqual({
    name: "Remote name 'copy' is already used",
  });
});

test("confirm_remove_remote_literal", async () => {
  expect(removeMessage(ORIGIN)).toBe("Remove remote origin 'https://example.com/a.git'?");
  const fake = api();
  render(RemotesDialog, { api: fake, onclose: vi.fn() });

  await fireEvent.click(await screen.findByRole("row", { name: /origin/ }));
  await fireEvent.click(screen.getByRole("button", { name: "Remove" }));

  const confirm = await screen.findByRole("dialog", { name: "Remove Remote" });
  expect(within(confirm).getByText("Remove remote origin 'https://example.com/a.git'?")).toBeTruthy();
  expect(fake.remove).not.toHaveBeenCalled();

  await fireEvent.click(within(confirm).getByRole("button", { name: "Yes" }));
  await waitFor(() => expect(fake.remove).toHaveBeenCalledWith("origin"));
});

test("removing_and_saying_no_touches_nothing", async () => {
  const fake = api();
  render(RemotesDialog, { api: fake, onclose: vi.fn() });

  await fireEvent.click(await screen.findByRole("row", { name: /copy/ }));
  await fireEvent.click(screen.getByRole("button", { name: "Remove" }));
  const confirm = await screen.findByRole("dialog", { name: "Remove Remote" });
  await fireEvent.click(within(confirm).getByRole("button", { name: "No" }));

  expect(fake.remove).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog", { name: "Remove Remote" })).toBeNull();
});

test("the_table_shows_name_and_url_in_list_order", async () => {
  render(RemotesDialog, { api: api(), onclose: vi.fn() });

  await screen.findByRole("row", { name: /origin/ });
  const rows = screen.getAllByRole("row");
  const cells = (row: HTMLElement) => Array.from(row.children, (cell) => cell.textContent).join(" ");
  expect(rows.map(cells)).toEqual([
    "Name URL",
    "origin https://example.com/a.git",
    "copy /srv/copy.git",
  ]);
  expect(screen.getByRole("dialog", { name: "Git Remotes" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
});

test("add_proposes_origin_if_it_does_not_exist_and_adds_after_testing_the_url", async () => {
  const fake = api({ list: vi.fn(async () => []) });
  render(RemotesDialog, { api: fake, onclose: vi.fn() });
  await fireEvent.click(await screen.findByRole("button", { name: "Add" }));

  const define = await screen.findByRole("dialog", { name: "Define Remote" });
  expect((within(define).getByLabelText("Name:") as HTMLInputElement).value).toBe("origin");
  await fireEvent.input(within(define).getByLabelText("URL:"), { target: { value: "/srv/a.git" } });
  await fireEvent.click(within(define).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(fake.add).toHaveBeenCalledWith("origin", "/srv/a.git"));
  expect(fake.testUrl).toHaveBeenCalledWith("/srv/a.git", "origin");
});

test("add_with_origin_already_defined_proposes_no_name", async () => {
  render(RemotesDialog, { api: api(), onclose: vi.fn() });
  await fireEvent.click(await screen.findByRole("button", { name: "Add" }));

  const define = await screen.findByRole("dialog", { name: "Define Remote" });
  expect((within(define).getByLabelText("Name:") as HTMLInputElement).value).toBe("");
});

test("edit_sends_the_old_name_the_new_one_and_the_url", async () => {
  const fake = api();
  render(RemotesDialog, { api: fake, onclose: vi.fn() });
  await fireEvent.dblClick(await screen.findByRole("row", { name: /copy/ }));

  const define = await screen.findByRole("dialog", { name: "Define Remote" });
  await fireEvent.input(within(define).getByLabelText("Name:"), { target: { value: "other" } });
  await fireEvent.click(within(define).getByRole("button", { name: "OK" }));

  await waitFor(() => expect(fake.edit).toHaveBeenCalledWith("copy", "other", "/srv/copy.git"));
});

test("a_failure_on_add_shows_as_an_error_and_does_not_close_the_list", async () => {
  const fake = api({
    add: vi.fn(async () => {
      throw { kind: "failed", message: "error: remote origin already exists." };
    }),
  });

  const added = await addRemote(fake, "origin", "https://user:secret@host/a.git");

  expect(added).toBe(false);
  const [error] = shown();
  expect(error.kind).toBe("error");
  expect(error.title).toBe("Add Remote");
  expect(error.body).toContain("Cannot add remote origin 'https://user:***@host/a.git'");
  expect(error.body).toContain("remote origin already exists");
  expect(error.body).not.toContain("secret");
});

test("editing_without_changes_launches_nothing_and_removing_warns_on_failure", async () => {
  const fake = api({
    remove: vi.fn(async () => {
      throw { kind: "failed", message: "fatal: no such remote" };
    }),
  });

  expect(await editRemote(fake, ORIGIN, "origin", ORIGIN.url)).toBe(true);
  expect(fake.edit).not.toHaveBeenCalled();
  expect(await removeRemote(fake, ORIGIN)).toBe(false);
  expect(shown()[0].title).toBe("Remove Remote");
});

test("clone_opens_the_repository_when_done", async () => {
  const fake = api();
  const open = vi.fn(async () => {});

  await cloneFlow({ current: () => null, open }, fake)("https://example.com/a.git", "/work/copy");

  expect(fake.clone).toHaveBeenCalledWith("https://example.com/a.git", "/work/copy");
  expect(open).toHaveBeenCalledWith("/work/copy");
  expect(fake.openInNewWindow).not.toHaveBeenCalled();
});

test("clone_with_an_open_repository_asks_for_another_window", async () => {
  const fake = api();
  const open = vi.fn(async () => {});

  await cloneFlow({ current: () => "/repo/a", open }, fake)("/srv/a.git", "/work/copy");

  expect(fake.openInNewWindow).toHaveBeenCalledWith("/work/copy");
  expect(open).not.toHaveBeenCalled();
});

test("a_failing_clone_warns_and_opens_nothing", async () => {
  const fake = api({
    clone: vi.fn(async () => {
      throw { kind: "failed", message: "fatal: repository '/x' does not exist" };
    }),
  });
  const open = vi.fn(async () => {});

  await cloneFlow({ current: () => null, open }, fake)("/x", "/work/x");

  expect(open).not.toHaveBeenCalled();
  expect(shown()[0]).toMatchObject({ kind: "error", title: "Clone failed" });
  expect(shown()[0].body).toContain("does not exist");
});

test("a_complete_clone_with_a_broken_submodule_is_opened_and_warns_as_a_warning", async () => {
  const fake = api({
    clone: vi.fn(async () => ({ path: "/work/copy", warning: "fatal: submodule 'sub' failed" })),
  });
  const open = vi.fn(async () => {});

  await cloneFlow({ current: () => null, open }, fake)("/srv/a.git", "/work/copy");

  expect(open).toHaveBeenCalledWith("/work/copy");
  expect(shown()).toHaveLength(1);
  expect(shown()[0]).toMatchObject({ kind: "warning", title: "Cloned, but a submodule failed" });
  expect(shown()[0].body).toContain("submodule 'sub' failed");
});

test("a_cancelled_clone_does_not_warn", async () => {
  const fake = api({
    clone: vi.fn(async () => {
      throw { kind: "cancelled", message: "The operation was cancelled" };
    }),
  });

  await cloneFlow({ current: () => null, open: vi.fn() }, fake)("/x", "/work/x");

  expect(shown()).toEqual([]);
});

test("the_clone_dialog_asks_for_url_and_directory", async () => {
  const onanswer = vi.fn();
  render(CloneDialog, { onanswer });
  const clone = screen.getByRole("button", { name: "Clone" }) as HTMLButtonElement;
  expect(clone.disabled).toBe(true);

  await fireEvent.input(screen.getByLabelText("URL:"), { target: { value: " /srv/a.git " } });
  await fireEvent.input(screen.getByLabelText("Directory:"), { target: { value: "/work/a" } });
  await fireEvent.click(clone);

  expect(screen.getByRole("dialog", { name: "Clone Repository" })).toBeTruthy();
  expect(onanswer).toHaveBeenCalledWith({ url: "/srv/a.git", directory: "/work/a" });
});

test("init_creates_and_opens_in_this_window_if_there_is_no_repository", async () => {
  const fake = api();
  const open = vi.fn(async () => {});

  await initFlow({ current: () => null, open }, { confirm: vi.fn() }, fake)("/work/new");

  expect(fake.init).toHaveBeenCalledTimes(1);
  expect(fake.init).toHaveBeenCalledWith("/work/new", false);
  expect(open).toHaveBeenCalledWith("/work/new");
});

test("init_in_a_folder_under_git_asks_with_intellijs_text", async () => {
  const init = vi
    .fn()
    .mockResolvedValueOnce({ kind: "underGit" })
    .mockResolvedValueOnce({ kind: "created", path: "/repo/sub" });
  const fake = api({ init });
  const confirm = vi.fn(async () => true);
  const open = vi.fn(async () => {});

  await initFlow({ current: () => null, open }, { confirm }, fake)("/repo/sub");

  expect(confirm).toHaveBeenCalledWith({
    title: "Git Init",
    message:
      "The selected directory /repo/sub is already under Git.\nAre you sure that you want to create a new VCS root?",
  });
  expect(init).toHaveBeenLastCalledWith("/repo/sub", true);
  expect(open).toHaveBeenCalledWith("/repo/sub");
});

test("init_and_saying_no_creates_nothing", async () => {
  const fake = api({ init: vi.fn(async () => ({ kind: "underGit" as const })) });
  const open = vi.fn();

  await initFlow({ current: () => null, open }, { confirm: async () => false }, fake)("/repo/sub");

  expect(fake.init).toHaveBeenCalledTimes(1);
  expect(open).not.toHaveBeenCalled();
});

test("init_with_another_repository_open_asks_for_another_window_and_on_the_same_does_nothing", async () => {
  const fake = api();

  await initFlow({ current: () => "/repo/a", open: vi.fn() }, { confirm: vi.fn() }, fake)("/work/b");
  expect(fake.openInNewWindow).toHaveBeenCalledWith("/work/b");

  const same = api();
  await initFlow({ current: () => "/repo/a", open: vi.fn() }, { confirm: vi.fn() }, same)("/repo/a");
  expect(same.openInNewWindow).not.toHaveBeenCalled();
});

test("the_init_dialog_proposes_the_current_repository", async () => {
  const onanswer = vi.fn();
  render(InitDialog, { initialDirectory: "/repo/a", onanswer });

  expect(screen.getByRole("dialog", { name: "Create Git Repository" })).toBeTruthy();
  expect(screen.getByText("Select directory where the new Git repository will be created.")).toBeTruthy();
  await fireEvent.click(screen.getByRole("button", { name: "Create" }));

  expect(onanswer).toHaveBeenCalledWith("/repo/a");
});

test("open_exclude_says_nothing_if_it_does_not_exist_and_warns_on_failure", async () => {
  await openExclude(api({ openExclude: vi.fn(async () => false) }));
  expect(shown()).toEqual([]);

  await openExclude(
    api({
      openExclude: vi.fn(async () => {
        throw { kind: "failed", message: "Could not open" };
      }),
    }),
  );
  expect(shown()[0]).toMatchObject({ kind: "error", title: "Open .git/info/exclude" });
});
