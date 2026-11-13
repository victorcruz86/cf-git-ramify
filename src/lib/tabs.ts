import { STRIPE_ORDER, toolWindow, type OnFirstUseWindowId, type ToolWindowId } from "../shell/toolWindows";
import type { CommitMode } from "./api";

export type { ToolWindowId };

// What a tab of the `Git` and `Commit` tool windows shows (`ChangesViewContentManager`). Shelf and Stash
// exist only while they have entries.
export type ContentId = "local-changes" | "shelf" | "stash" | "log" | "console" | "terminal";

// Whoever asks to show something also says "commit" for wherever Local Changes is at the moment.
export type TabRequest = ContentId | "commit";

// The `weight` of each content in IntelliJ: lighter first.
export const CONTENT_WEIGHT: Record<ContentId, number> = {
  "local-changes": 10,
  shelf: 40,
  stash: 41,
  log: 50,
  console: 60,
  // Not a content of IntelliJ's Git window: its Terminal is a tool window of its own. Ramify keeps
  // it as the last tab of the Git window.
  terminal: 70,
};

// `VcsBundle` `Local Changes` and the tab names of `ChangesViewContentManager.kt:315-321`.
export const CONTENT_TITLE: Record<ContentId, string> = {
  "local-changes": "Local Changes",
  shelf: "Shelf",
  stash: "Stash",
  log: "Log",
  console: "Console",
  terminal: "Terminal",
};

// `CommitTabTitleUpdater.updateTab` in the New UI: in the `Commit` window, Local Changes is `Commit`
// (`tab.title.commit`), and the strip only shows with more than one content.
export function contentTitle(id: ContentId, window: ToolWindowId): string {
  return id === "local-changes" && window === "commit" ? "Commit" : CONTENT_TITLE[id];
}

export function orderContents(ids: ContentId[]): ContentId[] {
  return [...ids].sort((a, b) => CONTENT_WEIGHT[a] - CONTENT_WEIGHT[b]);
}

// The contents flagged `isInCommitToolWindow` live in the `Commit` window while it exists, which is only in
// the non-modal mode. Otherwise everything is in the `Git` window.
const IN_COMMIT_WINDOW: ContentId[] = ["local-changes", "shelf", "stash"];

// Per window, the contents of `ChangesViewContentManager` it holds, or `null` when it does not exist now.
// The windows with other contents (Project, Changes, Repositories, Pull Requests) hold none of these: `[]` while
// they exist.
export type WindowContents = Record<ToolWindowId, ContentId[] | null>;

// Per window, the content it shows; `null` for the windows that hold none.
export type SelectedContents = Record<ToolWindowId, ContentId | null>;

export function toolWindows(
  mode: CommitMode,
  available: ContentId[],
  registered: readonly OnFirstUseWindowId[] = [],
): WindowContents {
  const ordered = orderContents(available);
  const own = (id: ToolWindowId): ContentId[] | null =>
    !toolWindow(id).registeredOnFirstUse || registered.includes(id as OnFirstUseWindowId) ? [] : null;
  return {
    project: own("project"),
    commit: mode === "modal" ? null : ordered.filter((id) => IN_COMMIT_WINDOW.includes(id)),
    changes: own("changes"),
    repositories: own("repositories"),
    pulls: own("pulls"),
    git: mode === "modal" ? ordered : ordered.filter((id) => !IN_COMMIT_WINDOW.includes(id)),
  };
}

// The stripe: one button per window that exists now, in the registry's order.
export function stripeWindows(windows: WindowContents): ToolWindowId[] {
  return STRIPE_ORDER.filter((id) => windows[id] !== null);
}

// A selected content that is gone (the last stash dropped) leaves its window on the first one. The same
// record comes back when nothing vanished.
export function keepSelected(windows: WindowContents, selected: SelectedContents): SelectedContents {
  const kept = (id: ToolWindowId) => {
    const contents = windows[id];
    const chosen = selected[id];
    return !contents || contents.length === 0 || (chosen !== null && contents.includes(chosen))
      ? chosen
      : contents[0];
  };
  const next = Object.fromEntries(STRIPE_ORDER.map((id) => [id, kept(id)])) as SelectedContents;
  return STRIPE_ORDER.every((id) => next[id] === selected[id]) ? selected : next;
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
