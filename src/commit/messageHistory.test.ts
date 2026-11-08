import { expect, test, vi } from "vitest";
import { historyMenu } from "./messageHistory";

test("history_menu_lists_the_messages_in_the_order_given_and_chooses_one", () => {
  const choose = vi.fn();
  const items = historyMenu(["newest", "older"], choose);
  expect(items.map((item) => item && "label" in item && item.label)).toEqual(["newest", "older"]);
  const second = items[1];
  if (second && "run" in second) second.run?.();
  expect(choose).toHaveBeenCalledWith("older");
});

test("history_menu_without_messages_has_one_disabled_item", () => {
  const items = historyMenu([], vi.fn());
  expect(items).toHaveLength(1);
  expect(items[0]).toMatchObject({ label: "History is empty", disabled: true });
});
