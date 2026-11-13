import { get } from "svelte/store";
import { afterEach, expect, test, vi } from "vitest";
import { describeChange, externalNotifier, type ExternalChange } from "./external";
import { dismiss, notifications } from "./notifications";

afterEach(() => get(notifications).forEach(({ id }) => dismiss(id)));

const HASH = "def5678901234567890123456789012345678901";

test("each_change_reads_as_one_line", () => {
  const lines = (
    [
      { kind: "branchSwitched", from: "main", to: "side" },
      { kind: "headMoved", from: "abc12345", to: HASH },
      { kind: "stashCreated", message: "WIP on main: abc1234 first" },
      { kind: "stashRemoved", message: "On main: two" },
    ] as ExternalChange[]
  ).map(describeChange);

  expect(lines).toEqual([
    "Branch switched from 'main' to 'side'",
    "HEAD moved from abc12345 to def56789 (reset, rebase or amend)",
    "Stash created: WIP on main: abc1234 first",
    "Stash applied or dropped: On main: two",
  ]);
});

test("the_notice_stays_offers_the_log_and_the_stash_and_joins_changes_within_two_seconds", () => {
  let now = 1_000;
  const showInLog = vi.fn();
  const showStash = vi.fn();
  const report = externalNotifier({ showInLog, showStash }, () => now);

  report([{ kind: "stashCreated", message: "WIP on main: x" }]);
  now += 1_500;
  report([{ kind: "headMoved", from: "abc12345", to: HASH }]);

  const shown = get(notifications);
  expect(shown).toHaveLength(1);
  expect(shown[0]).toMatchObject({
    kind: "warning",
    title: "Changes made outside Ramify",
    body: "Stash created: WIP on main: x\nHEAD moved from abc12345 to def56789 (reset, rebase or amend)",
  });
  expect(shown[0].actions?.map((action) => action.label)).toEqual(["Show in Log", "Show Stash"]);
  shown[0].actions?.[0].run();
  expect(showInLog).toHaveBeenCalledWith(HASH);
  shown[0].actions?.[1].run();
  expect(showStash).toHaveBeenCalled();

  now += 2_500;
  report([{ kind: "branchSwitched", from: "main", to: "side" }]);
  // Later than two seconds: a notice of its own, and the first one stays until closed.
  const later = get(notifications);
  expect(later).toHaveLength(2);
  const newest = later[1];
  expect(newest.body).toBe("Branch switched from 'main' to 'side'");
  expect(newest.actions?.map((action) => action.label)).toEqual(["Show in Log"]);
  newest.actions?.[0].run();
  expect(showInLog).toHaveBeenLastCalledWith(undefined);
});
