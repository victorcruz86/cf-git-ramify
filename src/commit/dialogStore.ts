import { writable } from "svelte/store";
import type { CommitMode } from "../lib/api";
import type { CheckinRequest } from "./checkin";
import type { Executor, ExecutorChanges } from "./dialog";

// `Commit and Rebase…` of Fixup… and Squash Into…: the base (`<hash>~`) and the HEAD that was seen when
// proposing it, which the executor is only valid for. Both or neither: a base without its HEAD cannot be
// checked.
export type RebaseExecutor = { rebaseFrom: string; head: string } | { rebaseFrom?: never; head?: never };

// What opens the Commit Changes dialog. The application mounts it while there is one.
export type CommitDialogInput = {
  request: CheckinRequest;
  executor?: Executor;
  // Fixup… and Squash Into…: the message.
  prefilled?: string;
  onexecute?: (changes: ExecutorChanges) => Promise<void>;
} & RebaseExecutor;

const store = writable<CommitDialogInput | null>(null);

export const commitDialog = { subscribe: store.subscribe };

// The dialog is modal: while it is open another request does nothing.
export function openCommitDialog(input: CommitDialogInput): void {
  store.update((open) => open ?? input);
}

export function closeCommitDialog(): void {
  store.set(null);
}

// What a commit entry point asks for: ⌘K, `Commit…`, `Commit File…`, the Git menu, the branches popup and
// Fixup… and Squash Into… all say it the same way.
export type CommitRequestInput = {
  request: CheckinRequest;
  prefilled?: string;
} & RebaseExecutor;

// What the application provides: the commit interface in use, and what the non-modal one does with a
// request (the Commit tool window, with `checkin` or `propose`).
export interface CommitRouting {
  mode: () => CommitMode;
  nonModal: (input: CommitRequestInput) => void | Promise<void>;
}

let routing: CommitRouting | null = null;

export function routeCommitRequests(next: CommitRouting | null): void {
  routing = next;
}

// The one place that decides where a commit goes (`CheckinActionUtil`): the modal Commit Changes dialog,
// or the Commit tool window in non-modal mode. Before the application routes anything it is modal.
export function requestCommit(input: CommitRequestInput): void | Promise<void> {
  if (routing && routing.mode() === "non-modal") return routing.nonModal(input);
  openCommitDialog({ ...input, executor: "commit" });
}
