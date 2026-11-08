import { writable } from "svelte/store";
import type { ListChoice } from "../patch/flow";
import { showShelf } from "./request";
import type { ListChooserRequest, ShelfTabUi } from "./flow";

// The questions of the Shelf tab as the dialogs of `ShelfDialogs.svelte`: one at a time, a new one
// counting the previous as cancelled.
export type ShelfDialogRequest =
  | { kind: "confirm"; title: string; message: string }
  | ({ kind: "chooseList" } & ListChooserRequest);

const open = writable<{ request: ShelfDialogRequest; settle: (answer: unknown) => void } | null>(null);
export const shelfDialog = { subscribe: open.subscribe };

let current: { settle: (answer: unknown) => void } | null = null;

function ask<T>(request: ShelfDialogRequest): Promise<T | null> {
  return new Promise((resolve) => {
    current?.settle(null);
    const entry = { request, settle: (answer: unknown) => resolve(answer as T | null) };
    current = entry;
    open.set(entry);
  });
}

export function answerShelfDialog(answer: unknown): void {
  const entry = current;
  current = null;
  open.set(null);
  entry?.settle(answer);
}

export const shelfDialogsUi: ShelfTabUi = {
  confirm: async (title, message) => (await ask<boolean>({ kind: "confirm", title, message })) ?? false,
  chooseList: (request) =>
    ask<{ list: ListChoice; removeApplied: boolean }>({ kind: "chooseList", ...request }),
  select: (name) => showShelf(name),
};
