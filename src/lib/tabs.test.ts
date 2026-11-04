import { expect, test, vi } from "vitest";
import { nextTab, onTabRequested, requestTab } from "./tabs";

test("shortcut_goes_to_its_tab", () => {
  expect(nextTab("commit", "log")).toBe("log");
  expect(nextTab("log", "commit")).toBe("commit");
});

test("shortcut_of_the_active_tab_toggles", () => {
  expect(nextTab("commit", "commit")).toBe("log");
  expect(nextTab("log", "log")).toBe("commit");
});

test("whoever_requests_a_tab_gets_it_from_whoever_listens", () => {
  const shown = vi.fn();
  const stop = onTabRequested(shown);

  requestTab("console");
  stop();
  requestTab("log");

  expect(shown).toHaveBeenCalledTimes(1);
  expect(shown).toHaveBeenCalledWith("console");
});
