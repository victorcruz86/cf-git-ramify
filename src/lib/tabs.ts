import type { CommitMode } from "./api";

// What a tab of the `Git` and `Commit` tool windows shows (`ChangesViewContentManager`). Shelf and Stash
// arrive with the shelf and stash views; the model already has them.
export type ContentId = "local-changes" | "shelf" | "stash" | "log" | "console";

// Whoever asks to show something also says "commit" for wherever Local Changes is at the moment.
export type TabRequest = ContentId | "commit";

export type ToolWindowId = "commit" | "git";

// The `weight` of each content in IntelliJ: lighter first.
export const CONTENT_WEIGHT: Record<ContentId, number> = {
  "local-changes": 10,
  shelf: 40,
  stash: 41,
  log: 50,
  console: 60,
};

// `VcsBundle` `Local Changes` and the tab names of `ChangesViewContentManager.kt:315-321`.
export const CONTENT_TITLE: Record<ContentId, string> = {
  "local-changes": "Local Changes",
  shelf: "Shelf",
  stash: "Stash",
  log: "Log",
  console: "Console",
};

export function orderContents(ids: ContentId[]): ContentId[] {
  return [...ids].sort((a, b) => CONTENT_WEIGHT[a] - CONTENT_WEIGHT[b]);
}

// The contents flagged `isInCommitToolWindow` live in the `Commit` window while it exists, which is only in
// the non-modal mode. Otherwise everything is in the `Git` window.
const IN_COMMIT_WINDOW: ContentId[] = ["local-changes", "shelf", "stash"];

export function toolWindows(
  mode: CommitMode,
  available: ContentId[],
): { commit: ContentId[] | null; git: ContentId[] } {
  const ordered = orderContents(available);
  if (mode === "modal") return { commit: null, git: ordered };
  return {
    commit: ordered.filter((id) => IN_COMMIT_WINDOW.includes(id)),
    git: ordered.filter((id) => !IN_COMMIT_WINDOW.includes(id)),
  };
}

// Whoever wants to show a tab (a notification with "Show console", for example) does not know
// the application: it requests it and the application shows it.
const requests = new Set<(tab: TabRequest) => void>();

export function onTabRequested(handler: (tab: TabRequest) => void): () => void {
  requests.add(handler);
  return () => requests.delete(handler);
}

export function requestTab(tab: TabRequest): void {
  requests.forEach((handler) => handler(tab));
}
