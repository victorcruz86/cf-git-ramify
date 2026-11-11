import { emitTo, listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

// Window labels, mirroring `src-tauri/src/windows.rs`: the first project window is `main` with
// `diff`, `merge`, `viewer` and `selection-history`; later ones are `main-<n>`, `diff-<n>`...
// with n >= 2 written without leading zeros. The owner of a label is its main window's label.
const KINDS = ["main", "diff", "merge", "viewer", "selection-history"] as const;

// The label of this page's window.
export const ownLabel = (): string => getCurrentWebviewWindow().label;

export function ownerOf(label: string): string | null {
  for (const kind of KINDS) {
    if (label === kind) return "main";
    const digits = label.startsWith(`${kind}-`) ? label.slice(kind.length + 1) : null;
    if (digits !== null && /^[1-9][0-9]*$/.test(digits)) {
      return Number(digits) >= 2 ? `main-${digits}` : null;
    }
  }
  return null;
}

// The five labels of the project window `owner` owns.
export function ownerGroup(owner: string): string[] {
  return KINDS.map((kind) => labelFor(kind, owner));
}

// The label of the window of `kind` that the project window `owner` owns.
export function labelFor(kind: (typeof KINDS)[number], owner: string): string {
  const suffix = owner.startsWith("main-") ? owner.slice("main".length) : "";
  return `${kind}${suffix}`;
}

// An event sent to one window (`emitTo`) also reaches every listener registered without a
// target, so the events of one window are listened to with its own label as the target.
// Every event of this app is per window and goes through `listenOwn` / `emitToOwner`, except
// `settings-changed`: Rust sends it to every window (`app.emit`) and its listener has no target.
// (`app-activated` and `app-deactivated` are sent to every main window, each of which hears them
// with `listenOwn`; `app-activated` says which project window was activated.)
export const listenOwn = <T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> =>
  listen<T>(event, handler, { target: ownLabel() });

// Sends an event to every window of this window's project. Listeners registered with `listenOwn`
// in another project's windows do not hear it; a listener registered without a target (`listen`)
// in any window would, because `emitTo` also reaches those.
export async function emitToOwner(event: string, payload?: unknown): Promise<void> {
  const owner = ownerOf(ownLabel());
  if (owner === null) return;
  await Promise.all(ownerGroup(owner).map((label) => emitTo(label, event, payload)));
}
