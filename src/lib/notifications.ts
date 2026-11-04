import { writable } from "svelte/store";

export interface Notification {
  kind: "info" | "warning" | "error";
  title: string;
  body?: string;
  actions?: { label: string; run: () => void }[];
  sticky?: boolean;
}

export interface Shown extends Notification {
  id: number;
}

// Informational ones go away on their own, like IntelliJ's balloons; a warning or an error stays
// until closed, and `sticky` does the same for an informational one.
export const INFO_TIMEOUT_MS = 10_000;

const store = writable<Shown[]>([]);
const timers = new Map<number, ReturnType<typeof setTimeout>>();
let lastId = 0;

export const notifications = { subscribe: store.subscribe };

export function notify(notification: Notification): number {
  const id = ++lastId;
  store.update((shown) => [...shown, { ...notification, id }]);
  if (notification.kind === "info" && !notification.sticky) {
    timers.set(
      id,
      setTimeout(() => dismiss(id), INFO_TIMEOUT_MS),
    );
  }
  return id;
}

export function dismiss(id: number): void {
  clearTimeout(timers.get(id));
  timers.delete(id);
  store.update((shown) => shown.filter((notification) => notification.id !== id));
}
