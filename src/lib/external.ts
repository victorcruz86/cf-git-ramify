import { dismiss, notifications, notify } from "./notifications";
import { get } from "svelte/store";

// Ramify's own `Notify about Git changes made outside Ramify` (Settings > Ramify,
// `src-tauri/src/external.rs`, `docs/parity-matrix.md`).
export type ExternalChange =
  | { kind: "branchSwitched"; from: string; to: string }
  | { kind: "headMoved"; from: string; to: string }
  | { kind: "stashCreated"; message: string }
  | { kind: "stashRemoved"; message: string };

export const EXTERNAL_TITLE = "Changes made outside Ramify";
// Changes reported this close together go in one notice.
const JOIN_MS = 2_000;

export function describeChange(change: ExternalChange): string {
  switch (change.kind) {
    case "branchSwitched":
      return `Branch switched from '${change.from}' to '${change.to}'`;
    case "headMoved":
      return `HEAD moved from ${change.from} to ${change.to.slice(0, 8)} (reset, rebase or amend)`;
    case "stashCreated":
      return `Stash created: ${change.message}`;
    case "stashRemoved":
      return `Stash applied or dropped: ${change.message}`;
  }
}

export interface ExternalHandlers {
  // The commit HEAD moved to, or the Log as it is after a branch switch.
  showInLog: (hash?: string) => void;
  showStash: () => void;
}

// A warning, which stays until closed: the point is to be seen. `now` is for the tests.
export function externalNotifier(handlers: ExternalHandlers, now: () => number = Date.now) {
  let last: { id: number; at: number; changes: ExternalChange[] } | null = null;
  return (changes: ExternalChange[]) => {
    if (changes.length === 0) return;
    const open = last !== null && get(notifications).some((shown) => shown.id === last?.id);
    const joined = open && last !== null && now() - last.at < JOIN_MS ? [...last.changes, ...changes] : changes;
    if (open && last !== null && joined !== changes) dismiss(last.id);
    const moved = [...joined].reverse().find((change) => change.kind === "headMoved");
    const inLog = joined.some((change) => change.kind === "headMoved" || change.kind === "branchSwitched");
    const stash = joined.some((change) => change.kind === "stashCreated" || change.kind === "stashRemoved");
    const actions = [
      ...(inLog ? [{ label: "Show in Log", run: () => handlers.showInLog(moved?.kind === "headMoved" ? moved.to : undefined) }] : []),
      ...(stash ? [{ label: "Show Stash", run: () => handlers.showStash() }] : []),
    ];
    const id = notify({
      kind: "warning",
      title: EXTERNAL_TITLE,
      body: joined.map(describeChange).join("\n"),
      actions,
    });
    last = { id, at: now(), changes: joined };
  };
}
