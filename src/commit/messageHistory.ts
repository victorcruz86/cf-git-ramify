import type { MenuItem } from "../lib/menu";

// `ShowMessageHistoryAction`: the popup of the recent commit messages, in the order given (newest
// first). Without any, one disabled item says so.
export function historyMenu(messages: string[], choose: (message: string) => void): MenuItem[] {
  if (messages.length === 0) return [{ label: "History is empty", disabled: true, run: () => {} }];
  return messages.map((text) => ({ label: text, run: () => choose(text) }));
}
