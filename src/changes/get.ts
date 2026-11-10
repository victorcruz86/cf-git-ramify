import { get, writable } from "svelte/store";
import {
  revisionGet,
  revisionGetCheck,
  snapshotsRestore,
  type ApiError,
  type GetItem,
  type GotRevision,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";
import { runLocal } from "../lib/tasks";

// `V:172`, `V:57-60`, `V:549`, `V:176-178`.
export const GET_TITLE = "Get Revision";
export const OVERWRITE_FILE = "Overwrite Modified File";
export const OVERWRITE_FILES = "Overwrite Modified Files";
export const SEVERAL_MODIFIED = "Some files were locally modified.";
export const LOADING_CONTENT = "Loading Content";
export const GET_CONTENT_TITLE = "Get Revision Content";
export const fileModifiedText = (path: string) => `File ${path} has been locally modified.`;
export const cannotLoadText = (message: string) => `Cannot load revision: ${message}`;
export const cannotSaveText = (message: string) => `Cannot save content: ${message}`;

// `ReplaceFileConfirmationDialog.createMessage`: the path cut to 40 characters.
const PATH_LENGTH = 40;

// `SplitByLetterPolicy.getPresentableName`: the name whole, and as much of the start and the end of
// the folder as fits, with up to three dots between them.
export function presentableName(path: string, count: number): string {
  if (count >= path.length) return path;
  const nameLength = path.length - path.lastIndexOf("/") - 1;
  if (count <= nameLength) return path.slice(path.length - count);
  const dots = Math.min(3, count - nameLength);
  const shown = count - dots;
  const left = Math.floor((shown - nameLength) / 2) + ((shown - nameLength) % 2);
  const right = shown - left;
  return path.slice(0, left) + ".".repeat(dots) + path.slice(path.length - right);
}

// The questions of `Get`: `Messages.showOkCancelDialog` with the warning icon, and
// `Messages.showMessageDialog` with the error icon (a write) or the information one (a read).
// `title`: whose dialog it is, `Get Revision` and `Get Revision Content` without it.
export type GetQuestion =
  | { kind: "confirm"; message: string; ok: string; title?: string }
  | { kind: "error"; message: string; icon: "error" | "information"; title?: string };

export interface GetUi {
  confirm(message: string, ok: string, title?: string): Promise<boolean>;
  error(message: string, icon: "error" | "information", title?: string): Promise<void>;
}

// The open question, which `GetDialog.svelte` shows, and how it is answered.
const question = writable<{ request: GetQuestion; answer: (value: boolean) => void } | null>(null);
export const getQuestion = { subscribe: question.subscribe };

// One question at a time: another one asked meanwhile is answered Cancel.
function ask(request: GetQuestion): Promise<boolean> {
  if (get(question)) return Promise.resolve(false);
  return new Promise((resolve) => question.set({ request, answer: resolve }));
}

export function answerGetQuestion(value: boolean): void {
  question.update((open) => {
    open?.answer(value);
    return null;
  });
}

export const getDialogsUi: GetUi = {
  confirm: (message, ok, title) => ask({ kind: "confirm", message, ok, title }),
  error: async (message, icon, title) => {
    await ask({ kind: "error", message, icon, title });
  },
};

const files = (count: number) => (count === 1 ? "1 file" : `${count} files`);
const quoted = (paths: string[]) => paths.map((path) => `"${path}"`).join(", ");

// Ramify's own texts: 241 says nothing after a Get, Ramify says where its copy is.
function report(got: GotRevision, root: string) {
  if (got.written.length > 0) {
    notify({
      kind: "info",
      title: `Get of ${files(got.written.length)}. A snapshot was saved.`,
      actions: [{ label: "Restore", run: () => void restore(got.copy, root) }],
    });
  }
  if (got.changed.length > 0) {
    const left = got.changed.length === 1 ? "was left as it is" : "were left as they are";
    notify({
      kind: "warning",
      title: `${quoted(got.changed)} changed after the snapshot was saved and ${left}.`,
    });
  }
}

async function restore(copy: string, root: string) {
  try {
    const previous = await snapshotsRestore(copy, root);
    notify({
      kind: "info",
      title: "Snapshot restored.",
      actions: [{ label: "Undo", run: () => void restore(previous, root) }],
    });
  } catch (failure) {
    notify({ kind: "error", title: errorText(failure) });
  }
}

// A write that failed after the copy: `details` says where the copy is and what was done.
function interruptedOf(failure: unknown): GotRevision | null {
  const details = (failure as { details?: Partial<GotRevision> } | null)?.details;
  return typeof details?.copy === "string"
    ? { copy: details.copy, written: details.written ?? [], changed: details.changed ?? [] }
    : null;
}

// `rollback`'s `Interrupted`: the snapshot sentence after 241's text.
const inSnapshot = (copy: string) => `Your previous work is in snapshot ${copy}`;

function failureText(failure: unknown): { message: string; icon: "error" | "information" } {
  const kind = (failure as ApiError | null)?.kind;
  const text = errorText(failure);
  const interrupted = interruptedOf(failure);
  if (kind === "load_failed") return { message: cannotLoadText(text), icon: "information" };
  if (kind === "save_failed" && interrupted) {
    return { message: `${cannotSaveText(text)}. ${inSnapshot(interrupted.copy)}`, icon: "error" };
  }
  if (kind === "save_failed") return { message: cannotSaveText(text), icon: "error" };
  return { message: text, icon: "error" };
}

export interface GetOptions {
  // The HEAD seen when the list was read: the write refuses if it moved (Review Focus 5).
  expectedHead?: string | null;
  ui?: GetUi;
  // Get from Branch (`CompareBranchesDiffPanel.GetVersionActionProvider`): the confirmation is
  // titled `Get from Branch '<name>'`, the progress `Loading Content from Branch…` and a failure
  // `Cannot Copy Changes`.
  titles?: { confirm: string; progress: string; error: string };
}

// `GetVersionAction.doGet`: the question when local files have changes (Cancel stops), then the
// write in the background as `Loading Content`. `null` when nothing was written.
export async function getRevision(
  items: GetItem[],
  { expectedHead = null, ui = getDialogsUi, titles }: GetOptions = {},
): Promise<GotRevision | null> {
  if (items.length === 0) return null;
  let root: string | null = null;
  try {
    const check = await revisionGetCheck(items);
    root = check.root;
    if (check.modified.length > 0) {
      const one = check.modified.length === 1;
      const message = one
        ? fileModifiedText(presentableName(`${check.root}/${check.modified[0]}`, PATH_LENGTH))
        : SEVERAL_MODIFIED;
      const ok = one ? OVERWRITE_FILE : OVERWRITE_FILES;
      const confirmed = titles
        ? ui.confirm(message, ok, titles.confirm)
        : ui.confirm(message, ok);
      if (!(await confirmed)) return null;
    }
    const got = await runLocal(titles?.progress ?? LOADING_CONTENT, "", () =>
      revisionGet(items, expectedHead ?? check.head, check.seen),
    );
    report(got, check.root);
    return got;
  } catch (failure) {
    const interrupted = interruptedOf(failure);
    if (interrupted && root !== null) report(interrupted, root);
    const { message, icon } = failureText(failure);
    await (titles ? ui.error(message, icon, titles.error) : ui.error(message, icon));
    return null;
  }
}
