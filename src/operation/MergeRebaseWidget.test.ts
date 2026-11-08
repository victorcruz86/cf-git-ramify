import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import type { Operation, RepoState } from "../lib/api";
import { TOKENS } from "../lib/theme";
import MergeRebaseWidget from "./MergeRebaseWidget.svelte";

afterEach(cleanup);

const state = (operation: Operation | null): RepoState => ({
  operation,
  branch: "main",
  label: "main",
  head: "a".repeat(40),
});

function show(operation: Operation | null, conflicts = false) {
  const handlers = { abort: vi.fn(), continue: vi.fn(), skip: vi.fn() };
  const onresolve = vi.fn();
  render(MergeRebaseWidget, { state: state(operation), conflicts, handlers, onresolve });
  return { handlers, onresolve };
}

const names = () => screen.queryAllByRole("button").map((button) => button.getAttribute("aria-label"));

test("nothing_shows_with_no_operation_in_progress", () => {
  const { container } = render(MergeRebaseWidget, {
    state: state(null),
    conflicts: false,
    handlers: { abort: vi.fn(), continue: vi.fn(), skip: vi.fn() },
    onresolve: vi.fn(),
  });

  expect(names()).toEqual([]);
  expect(container.querySelector(".widget")).toBeNull();
});

test("during_a_rebase_it_offers_continue_and_abort", () => {
  show("rebasing");

  expect(names()).toEqual(["Continue Rebase", "Abort Rebase"]);
});

test("during_a_rebase_with_conflicts_resolve_stays_hidden", () => {
  show("rebasing", true);

  expect(names()).toEqual(["Continue Rebase", "Abort Rebase"]);
});

test("during_a_merge_with_conflicts_it_offers_resolve_and_abort", () => {
  show("merging", true);

  expect(names()).toEqual(["Resolve Conflicts…", "Abort Merge"]);
});

test("a_merge_without_conflicts_only_offers_abort", () => {
  show("merging");

  expect(names()).toEqual(["Abort Merge"]);
});

test("cherry_pick_and_revert_have_their_own_abort", () => {
  const { unmount } = render(MergeRebaseWidget, {
    state: state("cherryPicking"),
    conflicts: false,
    handlers: { abort: vi.fn(), continue: vi.fn(), skip: vi.fn() },
    onresolve: vi.fn(),
  });
  expect(names()).toEqual(["Abort Cherry-Pick"]);
  unmount();

  show("reverting");
  expect(names()).toEqual(["Abort Revert"]);
});

test("each_button_runs_the_handler_of_the_git_menu", async () => {
  const { handlers, onresolve } = show("rebasing");
  await fireEvent.click(screen.getByRole("button", { name: "Continue Rebase" }));
  await fireEvent.click(screen.getByRole("button", { name: "Abort Rebase" }));
  cleanup();
  const merge = show("merging", true);
  await fireEvent.click(screen.getByRole("button", { name: "Resolve Conflicts…" }));
  expect(merge.onresolve).toHaveBeenCalledTimes(1);
  expect(onresolve).not.toHaveBeenCalled();

  expect(handlers.continue).toHaveBeenCalledTimes(1);
  expect(handlers.abort).toHaveBeenCalledWith("rebasing");
});

test("the_tooltip_is_the_text_of_the_action", () => {
  show("merging", true);

  expect(screen.getByRole("button", { name: "Abort Merge" }).getAttribute("title")).toBe("Abort Merge");
});

test("resolve_and_continue_are_running_colored_and_aborts_stop_colored", () => {
  show("merging", true);

  const background = (name: string) =>
    screen.getByRole("button", { name }).style.getPropertyValue("--button-background");
  expect(background("Resolve Conflicts…")).toBe("var(--run-running)");
  expect(background("Abort Merge")).toBe("var(--run-stop)");
  cleanup();
  show("rebasing");
  expect(background("Continue Rebase")).toBe("var(--run-running)");
  expect(background("Abort Rebase")).toBe("var(--run-stop)");
  // `RunWidget.runningBackground` and `RunWidget.stopBackground` of the installed theme.
  expect(TOKENS["run-running"]).toBe("#57965C");
  expect(TOKENS["run-stop"]).toBe("#C94F4F");
});
