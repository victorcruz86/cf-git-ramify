import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { PullInfo, PullOption } from "../lib/api";
import { dismiss, notifications } from "../lib/notifications";
import PullDialog from "./PullDialog.svelte";

const INFO: PullInfo = {
  branch: "main",
  remotes: ["origin", "upstream"],
  remoteBranches: ["origin/main", "origin/feature/x", "upstream/main", "upstream/dev"],
  tracked: ["origin", "main"],
};

type Reload = (remote: string) => Promise<PullInfo | null>;

function open(
  info: Partial<PullInfo> = {},
  options: PullOption[] = [],
  reload: Reload = vi.fn(async () => null),
) {
  const onanswer = vi.fn();
  render(PullDialog, { info: { ...INFO, ...info }, options, reload, onanswer });
  return { onanswer, reload };
}

const remote = () => screen.getByRole("combobox", { name: "Remote" }) as HTMLSelectElement;
const branch = () => screen.getByRole("combobox", { name: "Branch" }) as HTMLInputElement;
const pull = () => screen.getByRole("button", { name: "Pull" });
const modify = () => screen.getByRole("button", { name: "Modify options" });
const option = (flag: string) => screen.getByRole("checkbox", { name: flag }) as HTMLInputElement;

beforeEach(() => get(notifications).forEach(({ id }) => dismiss(id)));
afterEach(cleanup);

test("title_pull_to_branch", () => {
  open();
  expect(screen.getByRole("dialog", { name: "Pull to main" })).toBeTruthy();
  expect(pull().className).toContain("default");
  expect(screen.getByText("git pull")).toBeTruthy();
});

test("without_a_current_branch_the_title_is_pull", () => {
  open({ branch: null, tracked: null });
  expect(screen.getByRole("dialog", { name: "Pull" })).toBeTruthy();
});

test("tracked_remote_and_branch_by_default", async () => {
  const { onanswer } = open({ tracked: ["upstream", "dev"] });
  expect(remote().value).toBe("upstream");
  expect(branch().value).toBe("dev");
  expect(branch().placeholder).toBe("Specify branch");

  await fireEvent.click(pull());

  expect(onanswer).toHaveBeenCalledWith({ remote: "upstream", branch: "dev", options: [] });
});

test("without_a_tracked_one_uses_the_one_with_the_same_name", () => {
  open({ tracked: null, branch: "dev", remotes: ["upstream"] });
  expect(remote().value).toBe("upstream");
  expect(branch().value).toBe("dev");
});

test("without_remotes_the_combo_says_none", () => {
  open({ remotes: [], remoteBranches: [], tracked: null });
  expect(remote().selectedOptions[0].text).toBe("None");
});

test("when_changing_remote_the_branch_adjusts_to_that_remotes_branches", async () => {
  open({ tracked: null, branch: "dev" });
  expect(branch().value).toBe("");

  await fireEvent.change(remote(), { target: { value: "upstream" } });

  expect(branch().value).toBe("dev");
  const names = [...document.querySelectorAll("datalist option")].map((item) =>
    item.getAttribute("value"),
  );
  expect(names).toEqual(["main", "dev"]);
});

test("selection_errors", async () => {
  const { onanswer } = open({ remotes: [], remoteBranches: [], tracked: null });
  await fireEvent.click(pull());
  expect((await screen.findByRole("alert")).textContent).toBe("Select the remote to pull from");

  cleanup();
  const second = open({ tracked: null, branch: null });
  expect(branch().value).toBe("");
  await fireEvent.click(pull());
  expect((await screen.findByRole("alert")).textContent).toBe(
    "The branch to pull from should be selected.",
  );

  await fireEvent.input(branch(), { target: { value: "invented" } });
  expect((await screen.findByRole("alert")).textContent).toBe("There is no matching branch.");
  expect(onanswer).not.toHaveBeenCalled();
  expect(second.onanswer).not.toHaveBeenCalled();

  await fireEvent.input(branch(), { target: { value: "feature/x" } });
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  await fireEvent.click(pull());
  expect(second.onanswer).toHaveBeenCalledWith({
    remote: "origin",
    branch: "feature/x",
    options: [],
  });
});

test("errors_do_not_show_before_the_first_attempt", () => {
  open({ tracked: null, branch: null });
  expect(screen.queryByRole("alert")).toBeNull();
});

test("incompatible_options_greyed_out", async () => {
  open();
  expect(screen.queryByRole("checkbox", { name: "--rebase" })).toBeNull();
  await fireEvent.click(modify());

  expect(screen.getByText("Add Pull Options")).toBeTruthy();
  for (const flag of ["--rebase", "--ff-only", "--no-ff", "--squash", "--no-commit", "--no-verify"]) {
    expect(option(flag).disabled).toBe(false);
  }
  expect(screen.getByText("Bypass the pre-merge and commit message hooks")).toBeTruthy();

  await fireEvent.click(option("--squash"));

  expect(option("--squash").checked).toBe(true);
  expect(option("--squash").disabled).toBe(false);
  expect(option("--rebase").disabled).toBe(true);
  expect(option("--ff-only").disabled).toBe(true);
  expect(option("--no-ff").disabled).toBe(true);
  expect(option("--no-commit").disabled).toBe(false);
  expect(option("--no-verify").disabled).toBe(false);

  await fireEvent.click(option("--squash"));
  expect(option("--rebase").disabled).toBe(false);
});

test("chosen_options_show_as_removable_buttons_and_travel_in_the_answer", async () => {
  const { onanswer } = open({}, ["noVerify"]);
  expect(screen.getByText("--no-verify")).toBeTruthy();

  await fireEvent.click(modify());
  await fireEvent.click(option("--no-commit"));
  await fireEvent.click(screen.getByRole("button", { name: "Remove --no-verify" }));
  await fireEvent.click(pull());

  expect(onanswer).toHaveBeenCalledWith({ remote: "origin", branch: "main", options: ["noCommit"] });
});

test("reloading_branches_launches_fetch_of_the_remote", async () => {
  const fresh: PullInfo = { ...INFO, remoteBranches: [...INFO.remoteBranches, "origin/fresh"] };
  const reload = vi.fn(async () => fresh);
  open({}, [], reload);
  expect(screen.getByText("Press ⌘R to update branches")).toBeTruthy();

  await fireEvent.keyDown(branch(), { key: "r", metaKey: true });

  expect(reload).toHaveBeenCalledWith("origin");
  await waitFor(() => {
    const names = [...document.querySelectorAll("datalist option")].map((item) =>
      item.getAttribute("value"),
    );
    expect(names).toEqual(["main", "feature/x", "fresh"]);
  });
  expect(branch().value).toBe("main");
});

test("reloading_if_it_fails_leaves_the_branches_as_they_were", async () => {
  const reload = vi.fn(async () => null);
  open({}, [], reload);

  await fireEvent.keyDown(branch(), { key: "r", metaKey: true });

  await waitFor(() => expect(reload).toHaveBeenCalled());
  expect(branch().value).toBe("main");
});

test("reloading_without_a_remote_warns", async () => {
  const { reload } = open({ remotes: [], remoteBranches: [], tracked: null });

  await fireEvent.keyDown(branch(), { key: "r", metaKey: true });

  expect(reload).not.toHaveBeenCalled();
  expect(get(notifications).map(({ title, body }) => [title, body])).toEqual([
    ["Cannot load remote branches", "Select a local and a remote repository"],
  ]);
});

test("cancelling_answers_nothing", async () => {
  const { onanswer } = open();

  await fireEvent.click(screen.getByRole("button", { name: "Cancel" }));

  expect(onanswer).toHaveBeenCalledWith(null);
});

// WebKit opens a `<datalist>` as soon as its input has the focus, and the dialog focuses the branch
// field: the suggestions covered Cancel and Pull. Like `ComboBoxWithAutoCompletion`, they come with typing.
test("the_branch_suggestions_open_only_while_typing", async () => {
  open();
  expect(branch().getAttribute("list")).toBeNull();

  await fireEvent.input(branch(), { target: { value: "ma" } });
  expect(branch().getAttribute("list")).toBe("pull-branches");

  await fireEvent.blur(branch());
  expect(branch().getAttribute("list")).toBeNull();
});
