export type Tab = "commit" | "log" | "console";

export function nextTab(current: Tab, shortcut: "commit" | "log"): Tab {
  if (current !== shortcut) return shortcut;
  return current === "commit" ? "log" : "commit";
}

// Whoever wants to show a tab (a notification with "Show console", for example) does not know
// the application: it requests it and the application shows it.
const requests = new Set<(tab: Tab) => void>();

export function onTabRequested(handler: (tab: Tab) => void): () => void {
  requests.add(handler);
  return () => requests.delete(handler);
}

export function requestTab(tab: Tab): void {
  requests.forEach((handler) => handler(tab));
}
