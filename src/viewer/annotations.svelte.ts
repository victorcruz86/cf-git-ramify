import type { EditorView } from "@codemirror/view";
import { requestTitle } from "../compare/actions";
import {
  annotateRefresh,
  annotateStart,
  commitDetails,
  compareChangesBetween,
  compareLocalState,
  diffWindowOpenRequest,
  taskCancel,
  type AnnotatedCommit,
  type Annotation,
  type ApiError,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { taskResult } from "../lib/tasks";
import { commitDiffPairs } from "./annotation";
import { goToLine } from "./editor";
import type { ViewerTab } from "./model";

// What the controller reads of the viewer: its tabs and their editors.
export interface AnnotationHost {
  tab: (key: string) => ViewerTab | undefined;
  view: (key: string) => EditorView | undefined;
}

// `dialog.message.can.t.create.text.editor.for` (`V:945`): a file over the content limit opens in
// a preview, not a text editor, so `AnnotateLocalFileAction.perform` has nothing to annotate.
export const cannotCreateEditorText = (path: string) => `Cannot create text editor for ${path}`;

// A tab can be annotated with a text editor: a text that is not the preview of a larger file.
export const annotatable = (tab: ViewerTab | undefined) =>
  tab?.content?.kind === "text" && tab.content.previewOf === null;

// The annotations of the viewer's tabs (decision 7), as `AnnotateToggleAction` and
// `VcsAnnotationLocalChangesListenerImpl` keep them:
// - on: `Loading Annotations` (`V:538`), a task of its own that a newer request or closing cancels;
// - the annotation waits behind the notice `V:687` when its line count is suspicious;
// - a new text of a local file is mapped again with the same blame, the gutter hidden meanwhile;
// - a change of the repository that moves the base revision, or that cannot be checked, closes it.
export class TabAnnotations {
  // The annotations computed, by tab.
  shown = $state.raw<Record<string, Annotation>>({});
  // `MyEditorNotificationPanel`: the tabs whose annotation waits for `Display anyway`.
  held = $state.raw<Record<string, true>>({});
  // The tabs whose text changed and is being mapped again: no gutter until it is.
  stale = $state.raw<Record<string, true>>({});
  // An annotation that could not be computed: its text, under `message.title.annotate` (`V:106`).
  failure = $state<string | null>(null);

  #host: AnnotationHost;
  // The tabs whose annotations are on, and those to annotate once their text is read.
  #on = new Set<string>();
  #wanted = new Set<string>();
  // The latest request of each tab and its task.
  #requests = new Map<string, number>();
  #tasks = new Map<string, number>();
  // The line `Annotate Revision` was asked from, for the tab it opens.
  #from = new Map<string, { text: string; line: number }>();

  constructor(host: AnnotationHost) {
    this.#host = host;
  }

  isOn(key: string): boolean {
    return this.#on.has(key);
  }

  // What the gutter shows now.
  visible(key: string): Annotation | null {
    if (this.held[key] || this.stale[key]) return null;
    return this.shown[key] ?? null;
  }

  // Annotated once its text is read: a target from another window, or the tab `Annotate Revision`
  // opens (with the line it was asked from).
  want(key: string, from: { text: string; line: number } | null = null) {
    this.#wanted.add(key);
    if (from) this.#from.set(key, from);
  }

  // The tab's text was read (again), or a want came for a tab that has one. While it still reads,
  // nothing happens: the read calls again, and the want waits for it.
  textArrived(key: string) {
    const tab = this.#host.tab(key);
    if (!tab?.content) return;
    const wanted = this.#wanted.delete(key);
    if (this.shown[key]) {
      if (!annotatable(tab)) this.close(key);
      // `AnnotateRevisionActionBase.annotate`: asked again, annotated again.
      else if (wanted) void this.start(key);
      else if (tab.revision === null) {
        this.stale = { ...this.stale, [key]: true };
        void this.refresh(key);
      }
      return;
    }
    if (wanted || this.#on.has(key)) void this.start(key);
  }

  // `repo-changed` with the local file's text as it was: the base revision may have moved.
  repositoryChanged(key: string) {
    if (this.shown[key]) void this.refresh(key);
  }

  toggle(key: string) {
    if (this.#on.has(key)) this.close(key);
    else void this.start(key);
  }

  async start(key: string) {
    const tab = this.#host.tab(key);
    if (!tab || tab.content?.kind !== "text") {
      this.#from.delete(key);
      return;
    }
    if (!annotatable(tab)) {
      this.#from.delete(key);
      this.failure = cannotCreateEditorText(tab.path);
      return;
    }
    this.#on.add(key);
    const request = this.#bump(key);
    const from = this.#from.get(key) ?? null;
    this.#from.delete(key);
    try {
      const id = await annotateStart({
        path: tab.path,
        revision: tab.revision,
        localText: tab.revision === null ? tab.content.text : null,
        from,
      });
      if (!this.#current(key, request)) {
        taskCancel(id).catch(() => {});
        return;
      }
      this.#tasks.set(key, id);
      const annotation = await taskResult<Annotation>(id);
      if (!this.#current(key, request)) return;
      this.#tasks.delete(key);
      this.shown = { ...this.shown, [key]: annotation };
      this.stale = without(this.stale, key);
      // `AnnotateToggleAction.doAnnotate`: more than one line apart, the notice instead (`V:687`).
      if (annotation.suspicious) this.held = { ...this.held, [key]: true };
      const view = this.#host.view(key);
      if (view && annotation.line !== null) goToLine(view, annotation.line);
    } catch (error) {
      if (!this.#current(key, request)) return;
      this.#tasks.delete(key);
      if ((error as ApiError)?.kind === "cancelled") return;
      this.close(key);
      this.failure = errorText(error);
    }
  }

  // `VcsAnnotationLocalChangesListenerImpl.processFile`: closed when the base revision moved, or
  // when that cannot be told; else the lines of the text shown are mapped again.
  async refresh(key: string) {
    const tab = this.#host.tab(key);
    const annotation = this.shown[key];
    if (!tab || !annotation || tab.revision !== null || tab.content?.kind !== "text") return;
    const request = this.#bump(key);
    try {
      const refreshed = await annotateRefresh({
        path: tab.path,
        revision: annotation.revision,
        blamedPath: annotation.path,
        owners: annotation.owners,
        localText: tab.content.text,
      });
      if (!this.#current(key, request)) return;
      if (refreshed.kind === "closed") {
        this.close(key);
        return;
      }
      const { lines, origins } = refreshed;
      const same =
        JSON.stringify(lines) === JSON.stringify(annotation.lines) &&
        JSON.stringify(origins) === JSON.stringify(annotation.origins);
      if (!same) this.shown = { ...this.shown, [key]: { ...annotation, lines, origins } };
      this.stale = without(this.stale, key);
    } catch {
      if (this.#current(key, request)) this.close(key);
    }
  }

  // `Display anyway` of the notice.
  displayAnyway(key: string) {
    this.held = without(this.held, key);
  }

  // `Close Annotations` (`EB:76`), the toggle again, `Hide` of the notice, or a closed tab.
  close(key: string) {
    this.#on.delete(key);
    this.#wanted.delete(key);
    this.#from.delete(key);
    this.#bump(key);
    if (this.shown[key]) this.shown = without(this.shown, key);
    this.held = without(this.held, key);
    this.stale = without(this.stale, key);
  }

  // A new request of the tab: the one before is dropped, its task cancelled.
  #bump(key: string): number {
    const request = (this.#requests.get(key) ?? 0) + 1;
    this.#requests.set(key, request);
    const running = this.#tasks.get(key);
    if (running !== undefined) {
      this.#tasks.delete(key);
      taskCancel(running).catch(() => {});
    }
    return request;
  }

  #current(key: string, request: number): boolean {
    return this.#requests.get(key) === request && this.#on.has(key);
  }
}

function without<T>(record: Record<string, T>, key: string): Record<string, T> {
  if (!(key in record)) return record;
  const { [key]: _gone, ...others } = record;
  return others;
}

// `git hash-object -t tree /dev/null`: what a root commit's changes are read against.
const EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

// `ShowDiffFromAnnotation`: the line's commit in the diff window, every file of it, on this one.
export async function showCommitDiff(commit: AnnotatedCommit): Promise<void> {
  const [details] = await commitDetails([commit.hash]);
  const parent = details?.parents[0] ?? null;
  const changes = await compareChangesBetween(parent ?? EMPTY_TREE, commit.hash, []);
  const { files, selected } = commitDiffPairs(commit, parent, changes);
  const pair = files[selected];
  if (!pair) return;
  const { root } = await compareLocalState(pair.right.path);
  const [left, right] = [
    pair.left.revision === null ? null : pair.left.path,
    pair.right.revision === null ? null : pair.right.path,
  ];
  await diffWindowOpenRequest({
    kind: "revisions",
    files,
    title: requestTitle(root, left ?? right, right ?? left),
    selected,
  });
}
