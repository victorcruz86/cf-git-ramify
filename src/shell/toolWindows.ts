import { get, writable, type Readable } from "svelte/store";
import type { ActionId } from "../lib/keymap";

// The tool windows of the stripe, in the order of the user's `window.layouts.xml`: `Project` and
// `Commit` on the left, `Git` (`Version Control`) at the bottom. `Changes` and `Repositories`
// are not in the layout: like `ChangesBrowserToolWindow` and `RepositoryBrowser`, they are
// registered on the left the first time something shows in them. `Pull Requests` is on the left
// too, while a remote points to a GitHub repository (`GHPRToolWindowFactory`).
export const TOOL_WINDOWS = [
  {
    id: "project",
    label: "Project",
    icon: "project",
    shortcut: "⌘1",
    action: "ActivateProjectToolWindow",
    side: "left",
    order: 0,
    registeredOnFirstUse: false,
    // `ActivateToolWindowAction`: the shortcut of the active window hides it.
    hidesWhenActive: true,
  },
  {
    id: "commit",
    label: "Commit",
    icon: "commit",
    shortcut: "⌘0",
    action: "ActivateCommitToolWindow",
    side: "left",
    order: 1,
    registeredOnFirstUse: false,
    // Phase 1's rule, kept: the Commit and Git windows are never hidden, rama has no empty area.
    hidesWhenActive: false,
  },
  {
    id: "changes",
    label: "Changes",
    icon: "changes",
    shortcut: null,
    action: null,
    side: "left",
    order: 2,
    registeredOnFirstUse: true,
    hidesWhenActive: true,
  },
  {
    id: "repositories",
    label: "Repositories",
    icon: "repositories",
    shortcut: null,
    action: null,
    side: "left",
    order: 3,
    registeredOnFirstUse: true,
    hidesWhenActive: true,
  },
  {
    // `toolwindow.stripe.Pull_Requests` (`GB:165`).
    id: "pulls",
    label: "Pull Requests",
    icon: "pullRequests",
    shortcut: null,
    action: null,
    side: "left",
    order: 4,
    registeredOnFirstUse: true,
    hidesWhenActive: true,
  },
  {
    // `ToolWindowChanges` is the branch icon in the new UI.
    id: "git",
    label: "Git",
    icon: "vcs",
    shortcut: "⌘9",
    action: "ActivateVersionControlToolWindow",
    side: "bottom",
    order: 0,
    registeredOnFirstUse: false,
    hidesWhenActive: false,
  },
] as const satisfies readonly {
  id: string;
  label: string;
  icon: string;
  shortcut: string | null;
  action: ActionId | null;
  side: "left" | "bottom";
  order: number;
  registeredOnFirstUse: boolean;
  hidesWhenActive: boolean;
}[];

export type ToolWindow = (typeof TOOL_WINDOWS)[number];
export type ToolWindowId = ToolWindow["id"];
export type OnFirstUseWindowId = Extract<ToolWindow, { registeredOnFirstUse: true }>["id"];

const SIDES = ["left", "bottom"] as const;

// The stripe: the left windows, then the bottom ones, each side by its order.
export const STRIPE_ORDER: readonly ToolWindowId[] = [...TOOL_WINDOWS]
  .sort((a, b) => SIDES.indexOf(a.side) - SIDES.indexOf(b.side) || a.order - b.order)
  .map((window) => window.id);

export const toolWindow = (id: ToolWindowId): ToolWindow =>
  TOOL_WINDOWS.find((window) => window.id === id)!;

// The window whose `Activate…ToolWindow` action a shortcut is.
export const windowOfAction = (action: ActionId | null): ToolWindowId | null =>
  TOOL_WINDOWS.find((window) => window.action !== null && window.action === action)?.id ?? null;

// The windows registered on first use that exist now. Opening another repository forgets them,
// as a new project has none.
const registered = writable<readonly OnFirstUseWindowId[]>([]);
export const registeredToolWindows: Readable<readonly OnFirstUseWindowId[]> = registered;

export function registerToolWindow(id: OnFirstUseWindowId): void {
  if (!get(registered).includes(id)) registered.update((ids) => [...ids, id]);
}

// A window that stops being available (the last GitHub remote removed) leaves the stripe.
export function unregisterToolWindow(id: OnFirstUseWindowId): void {
  if (get(registered).includes(id)) registered.update((ids) => ids.filter((other) => other !== id));
}

export function forgetToolWindows(): void {
  if (get(registered).length > 0) registered.set([]);
}

// What the shortcut or the stripe button of `id` does, with `active` in front and `previous` the
// one before it: a window that does not exist does nothing; another one comes to the front; the
// active one hides if it can, and what was in front before it comes back (in IntelliJ that is
// the window of the other side, which stayed open), else the Git window.
export function activation(
  id: ToolWindowId,
  active: ToolWindowId,
  previous: ToolWindowId | null,
  exists: (id: ToolWindowId) => boolean,
): ToolWindowId | null {
  if (!exists(id)) return null;
  if (active !== id) return id;
  if (!toolWindow(id).hidesWhenActive) return null;
  return previous !== null && previous !== id && exists(previous) ? previous : "git";
}

// Whoever wants a window in front (the Changes window showing a comparison) does not know the
// application: it asks, and the application brings it. Like `ToolWindow.activate`, a request
// never hides the window; only its shortcut and its stripe button do.
const activations = new Set<(id: ToolWindowId) => void>();

export function onWindowActivationRequested(handler: (id: ToolWindowId) => void): () => void {
  activations.add(handler);
  return () => activations.delete(handler);
}

export function activateWindow(id: ToolWindowId): void {
  activations.forEach((handler) => handler(id));
}
