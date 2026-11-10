<script lang="ts">
  import type { EditorView } from "@codemirror/view";
  import { onMount, tick, untrack } from "svelte";
  import { setWrap } from "../diff/editor";
  import { inGutter, loadWrap, softWrapItem, storeWrap, type DiffPlace } from "../diff/options";
  import {
    gitFileAction,
    gitMenuCurrentFile,
    hideWindow,
    onGitAction,
    onRepoChanged,
    onViewerOpen,
    showWindow,
    viewerContent,
    viewerStamp,
    viewerTargets,
    type AnnotatedCommit,
    type AnnotationTarget,
    type ViewerTarget,
  } from "../lib/api";
  import Dialog from "../lib/Dialog.svelte";
  import { CANNOT_OPEN, COPY_LINK, hostingItemsOf, hostingLinks, OPEN_IN_GITHUB } from "../hosting/links";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchViewerShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { trackTasks } from "../lib/tasks";
  import AnnotationGutter from "./AnnotationGutter.svelte";
  import { annotatable, showCommitDiff, TabAnnotations } from "./annotations.svelte";
  import { createViewerEditor, goToLine, linkLines, replaceText, selectedLines } from "./editor";
  import {
    absentText,
    binaryText,
    previewText,
    tabTitle,
    withoutTab,
    withTarget,
    type ViewerTab,
  } from "./model";
  import { provideViewerSelection } from "./window";
  import { openSelectionHistory } from "../history/window";
  import {
    canAnnotate,
    CURRENT_FILE_ACTIONS,
    currentFileItems,
    currentFileTitle,
    fileActionRun,
    gitSubmenu,
    readFileContext,
    runInPlace,
    type FileTarget,
    type GitFileActionId,
  } from "../files/gitFileMenu";

  // The read-only viewer (decision 2): one window, a tab per file and revision, tabs in rows
  // (`SCROLL_TAB_LAYOUT_IN_EDITOR=false`). Paths and contents are untrusted: shown as text only.
  const PLACE: DiffPlace = "Viewer";
  const EMPTY = { kind: "text", text: "", previewOf: null, previewSize: null } as const;
  // `annotation.wrong.line.number.notification.text` (`V:687`) with `{0}` = Git, without its HTML.
  const SUSPICIOUS_TEXT =
    "Number of lines annotated by Git is not equal to number of lines in the file. Check file encoding and line separators.";

  let tabs = $state.raw<ViewerTab[]>([]);
  let active = $state<string | null>(null);
  let wrap = loadWrap(PLACE);
  // `message.title.could.not.load.content` (`V:99`) shows this text.
  let failure = $state<string | null>(null);
  let menu = $state.raw<{ x: number; y: number; label: string; items: MenuItem[] } | null>(null);
  // A Git file action that failed here (a comparison, Show Current Revision): its text.
  let actionError = $state<string | null>(null);

  // Each tab's editor, made when its text arrives and kept, hidden, while another tab is active.
  const views = new Map<string, EditorView>();
  // The line a target asked for, applied once its text is in the editor.
  const lines = new Map<string, number>();
  // The last read of each tab: an older answer that arrives late is dropped.
  const reads = new Map<string, number>();
  // The tabs whose last read failed: the dialog is shown once, not on each later failure.
  const broken = new Set<string>();

  // Annotate (decision 7): each tab's annotations.
  const annotations = new TabAnnotations({
    tab: (key) => tabs.find((tab) => tab.key === key),
    view: (key) => views.get(key),
  });

  function patch(key: string, change: Partial<ViewerTab>) {
    tabs = tabs.map((tab) => (tab.key === key ? { ...tab, ...change } : tab));
  }

  async function load(key: string) {
    const tab = tabs.find((candidate) => candidate.key === key);
    if (!tab) return;
    const read = (reads.get(key) ?? 0) + 1;
    reads.set(key, read);
    // Taken before the read: a change made while it reads moves the stamp and is read next time.
    const stamp = tab.revision === null ? await viewerStamp(tab.path).catch(() => null) : null;
    try {
      const content = await viewerContent(tab.path, tab.revision);
      if (reads.get(key) !== read) return;
      broken.delete(key);
      patch(key, { content, failed: false, stamp });
      annotations.textArrived(key);
    } catch (error) {
      if (reads.get(key) !== read) return;
      patch(key, { content: EMPTY, failed: tab.revision !== null, stamp });
      if (!broken.has(key)) failure = errorText(error);
      broken.add(key);
    }
  }

  async function focusActive() {
    await tick();
    if (active) views.get(active)?.focus();
  }

  function add(target: ViewerTarget) {
    const { tabs: next, key } = withTarget(tabs, active, target);
    const created = next !== tabs;
    tabs = next;
    active = key;
    if (target.line !== null) lines.set(key, target.line);
    if (target.annotate) annotations.want(key);
    if (created) {
      void load(key);
      return;
    }
    if (target.annotate) annotations.textArrived(key);
    const view = views.get(key);
    const line = lines.get(key);
    if (view && line !== undefined) {
      lines.delete(key);
      goToLine(view, line);
    }
  }

  async function take() {
    const targets = await viewerTargets();
    targets.forEach(add);
    if (targets.length > 0) await focusActive();
  }

  function activate(key: string) {
    active = key;
    void focusActive();
  }

  async function close(key: string) {
    ({ tabs, active } = withoutTab(tabs, active, key));
    annotations.close(key);
    reads.delete(key);
    lines.delete(key);
    broken.delete(key);
    if (tabs.length === 0) await hideWindow().catch(() => {});
    else await focusActive();
  }

  // A local file follows the disk: on the watcher's notice it is read again only if its stamp
  // moved, so an unchanged 20 MiB file is not read for each notice. A revision does not change.
  // An annotated local file is checked either way: the read maps the new text again, and without
  // one the revision blamed may have moved.
  async function refresh() {
    for (const { key, path, revision, stamp } of tabs) {
      if (revision !== null) continue;
      const now = await viewerStamp(path).catch(() => undefined);
      if (!tabs.some((tab) => tab.key === key)) continue;
      if (now !== stamp) void load(key);
      else annotations.repositoryChanged(key);
    }
  }

  // `Annotate Revision` and `Annotate Previous Revision` (`AnnotateRevisionActionBase.annotate`):
  // the file at that revision in its tab, annotated, at the line asked from translated into it.
  function annotateRevision(fromKey: string, target: AnnotationTarget, line: number) {
    const from = tabs.find((tab) => tab.key === fromKey);
    if (from?.content?.kind !== "text") return;
    const { tabs: next, key } = withTarget(tabs, active, { path: target.path, revision: target.hash });
    const created = next !== tabs;
    tabs = next;
    active = key;
    annotations.want(key, { text: from.content.text, line });
    if (created) void load(key);
    else annotations.textArrived(key);
    void focusActive();
  }

  // `ShowDiffFromAnnotation`.
  function showDiff(commit: AnnotatedCommit) {
    showCommitDiff(commit).catch((error) => {
      actionError = errorText(error);
    });
  }

  // Makes the editor of `tab` when its text arrives; a later read with other text replaces it in
  // place, keeping the caret, the selection and the scroll.
  function editor(host: HTMLElement, tab: ViewerTab) {
    let view: EditorView | null = null;
    let shown: string | null = null;
    function show(next: ViewerTab) {
      const content = next.content;
      if (content?.kind !== "text" || content.text === shown) return;
      shown = content.text;
      if (view) {
        replaceText(view, content.text);
      } else {
        view = createViewerEditor(host, content.text, next.path, wrap);
        views.set(next.key, view);
        if (next.key === active) view.focus();
      }
      const line = lines.get(next.key);
      lines.delete(next.key);
      if (line !== undefined) goToLine(view, line);
    }
    show(tab);
    return {
      update: show,
      destroy() {
        view?.destroy();
        views.delete(tab.key);
      },
    };
  }

  function toggleWrap(on: boolean) {
    wrap = on;
    storeWrap(PLACE, on);
    setWrap([...views.values()], on);
  }

  // The gutter popup of `EditorGutterPopupMenu`: `Annotate with Git Blame` (`Close Annotations`
  // while they are shown), then `Soft-Wrap`; the annotations' own gutter has its popup. In the
  // text, the editor popup with what Ramify has of it: `Git ▸` (`VcsGroup` → `Git.ContextMenu`).
  async function contextmenu(event: MouseEvent) {
    event.preventDefault();
    const [x, y] = [event.clientX, event.clientY];
    if (inGutter(event)) {
      if (event.target instanceof Element && event.target.closest(".cm-annotations")) return;
      const key = active;
      if (key === null) return;
      const toggle: MenuItem = annotations.isOn(key)
        ? { label: "Close Annotations", run: () => annotations.close(key) }
        : await annotateItem(key);
      menu = { x, y, label: "Editor gutter", items: [toggle, null, softWrapItem(wrap, toggleWrap)] };
      return;
    }
    try {
      const { selection, state } = await fileContext();
      const { open, copy } = await githubItems();
      // `Copy / Paste Special` and `Open In` come before the version control group.
      menu = {
        x,
        y,
        label: "Editor",
        items: [
          ...(copy.length > 0 ? [{ label: "Copy / Paste Special", submenu: copy }] : []),
          ...(open.length > 0 ? [{ label: "Open In", submenu: open }] : []),
          gitSubmenu(selection, state),
        ],
      };
    } catch (error) {
      actionError = errorText(error);
    }
  }

  // `Open In ▸ GitHub` and `Copy / Paste Special ▸ Copy Link to GitHub Repository` of the active tab:
  // its file at the last commit in HEAD that touched it, or at its own revision, with the selected
  // lines of a text (`linkLines`).
  async function githubItems(): Promise<{ open: MenuItem[]; copy: MenuItem[] }> {
    const tab = tabs.find((candidate) => candidate.key === active);
    if (!tab) return { open: [], copy: [] };
    const view = views.get(tab.key);
    const lines = view && tab.content?.kind === "text" ? linkLines(view) : null;
    const target = { kind: "file", path: tab.path, lines, revision: tab.revision } as const;
    const fail = (error: unknown) => (actionError = `${CANNOT_OPEN}: ${errorText(error)}`);
    // One look-up (`git remote`, `git log`) for both submenus.
    const links = await hostingLinks(target);
    return {
      open: hostingItemsOf(links, "open", OPEN_IN_GITHUB, fail),
      copy: hostingItemsOf(links, "copy", COPY_LINK, fail),
    };
  }

  // `Annotate with Git Blame` while off: a file at a revision (`AnnotateVcsVirtualFileAction`) or a
  // local file in the repository (`AnnotateLocalFileAction`), with a text.
  async function annotateItem(key: string): Promise<MenuItem> {
    let can = false;
    try {
      can = (await fileContext()).state.annotation?.can ?? false;
    } catch {
      // Without the status, it stays off.
    }
    const label = can ? "Annotate with Git Blame" : "Annotate";
    return { label, disabled: !can, run: () => void annotations.start(key) };
  }

  // The local file of the active tab; a file at a revision is not one (`isInLocalFileSystem`).
  function localFile(): string | null {
    const tab = tabs.find((candidate) => candidate.key === active);
    return tab && tab.revision === null ? tab.path : null;
  }

  // The comparisons and Show Current Revision open from here, and Annotate toggles the active
  // tab's; the others (Commit File…, Add, Show History, Rollback…, Resolve Conflicts…) are the
  // main window's.
  function runFileAction(id: GitFileActionId, files: FileTarget[]) {
    if (id === "Annotate") {
      if (active) annotations.toggle(active);
      return;
    }
    if (id === "Vcs.ShowHistoryForBlock") {
      showSelectionHistory();
      return;
    }
    runInPlace(id, files)
      .then((done) => {
        if (!done) return gitFileAction({ id, paths: files.map((target) => target.path) });
      })
      .catch((error) => (actionError = errorText(error)));
  }

  // `SelectedBlockHistoryAction.showHistoryForSelection`: the selected lines of the active local
  // file, or its caret line, with the text shown.
  function showSelectionHistory() {
    const tab = tabs.find((candidate) => candidate.key === active);
    const view = active ? views.get(active) : undefined;
    if (!tab || !view || tab.revision !== null || tab.content?.kind !== "text") return;
    const { startLine, endLine } = selectedLines(view);
    openSelectionHistory(tab.path, startLine, endLine, tab.content.text).catch(
      (error) => (actionError = errorText(error)),
    );
  }

  // The file of the active tab for the Git file menu, and whether it can be annotated now.
  async function fileContext() {
    const tab = tabs.find((candidate) => candidate.key === active);
    const context = await readFileContext(localFile(), runFileAction, tab?.content?.kind === "binary");
    // `AnnotateVcsVirtualFileAction` and `AnnotateLocalFileAction` both need a text editor.
    const can =
      !!tab && annotatable(tab) && (tab.revision !== null || canAnnotate(context.selection.files));
    context.state.annotation = { can, on: !!tab && annotations.isOn(tab.key) };
    context.state.textEditor = !!tab && annotatable(tab) && views.has(tab.key);
    return context;
  }

  // While this window has the focus, `Git > Current File` is its file (`GitFileActionGroup`).
  let focused = $state(document.hasFocus());
  async function syncCurrentFile() {
    if (!document.hasFocus()) return;
    try {
      const { selection, state } = await fileContext();
      if (!document.hasFocus()) return;
      await gitMenuCurrentFile(currentFileTitle(selection), currentFileItems(selection, state));
    } catch {
      // The menu stays as it was; the next change tries again.
    }
  }

  $effect(() => {
    void [active, focused];
    untrack(() => void syncCurrentFile());
  });

  // An entry of `Git > Current File` chosen while this window had the focus. Anything else is the
  // main window's, whatever reaches this one.
  async function gitAction(id: string) {
    if (!(CURRENT_FILE_ACTIONS as string[]).includes(id) || !document.hasFocus()) return;
    if (menu || failure || actionError || annotations.failure) return;
    try {
      const { selection, state } = await fileContext();
      fileActionRun(selection, state, id)?.();
    } catch (error) {
      actionError = errorText(error);
    }
  }

  function keydown(event: KeyboardEvent) {
    if (menu || failure || actionError || annotations.failure) return;
    if (matchViewerShortcut(event) !== "CloseContent") return;
    event.preventDefault();
    if (active) void close(active);
  }

  onMount(() => {
    const stopSelection = provideViewerSelection(() => {
      const tab = tabs.find((candidate) => candidate.key === active);
      const view = active ? views.get(active) : undefined;
      if (!tab || !view) return null;
      return { path: tab.path, revision: tab.revision, ...selectedLines(view) };
    });
    const stops = [
      onViewerOpen(() => void take().catch(showFailure)),
      onRepoChanged(() => {
        void refresh();
        void syncCurrentFile();
      }),
      onGitAction((id) => void gitAction(id)),
      trackTasks(),
    ];
    (async () => {
      try {
        await Promise.all(stops);
        await take();
      } catch (error) {
        showFailure(error);
      } finally {
        await showWindow();
      }
    })();
    return () => {
      stopSelection();
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });

  function showFailure(error: unknown) {
    failure = errorText(error);
  }
</script>

<svelte:window
  onkeydown={keydown}
  onfocus={() => (focused = true)}
  onblur={() => (focused = false)}
/>

<main>
  <div class="tabs rows" role="tablist" aria-label="Files">
    {#each tabs as tab (tab.key)}
      {@const title = tabTitle(tab)}
      <div
        role="tab"
        class="tab"
        class:active={tab.key === active}
        aria-selected={tab.key === active}
        tabindex="-1"
        title={tab.path}
        onclick={() => activate(tab.key)}
        onkeydown={(event) => event.key === "Enter" && activate(tab.key)}
      >
        <span class="file-icon">
          <Icon name="text" />
          <span class="lock" title="read-only"><Icon name="locked" /></span>
        </span>
        <span class="name">{title}</span>
        <button
          type="button"
          class="close"
          aria-label="Close {title}"
          onclick={(event) => {
            event.stopPropagation();
            void close(tab.key);
          }}
        >
          <Icon name="close" size={14} />
        </button>
      </div>
    {/each}
  </div>

  {#each tabs as tab (tab.key)}
    <section class="file" class:hidden={tab.key !== active} aria-label={tabTitle(tab)}>
      {#if tab.content?.kind === "text"}
        {#if tab.content.previewOf !== null && tab.content.previewSize !== null}
          <p class="notice" role="status">
            {previewText(tab.content.previewOf, tab.content.previewSize)}
          </p>
        {/if}
        {#if annotations.held[tab.key]}
          <!-- `MyEditorNotificationPanel`: `V:687` (its HTML as text), `V:946-947`, `V:1008`. -->
          <div class="notice error" role="alert">
            <span>{SUSPICIOUS_TEXT}</span>
            <button type="button" class="link" onclick={() => annotations.displayAnyway(tab.key)}>
              Display anyway
            </button>
            <button
              type="button"
              class="link"
              title="Hide this notification"
              onclick={() => annotations.close(tab.key)}
            >
              Hide
            </button>
          </div>
        {/if}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="host" use:editor={tab} oncontextmenu={contextmenu}></div>
        <!-- The editor is made when the text arrives, before any annotation: `views` holds it. -->
        {#if annotations.visible(tab.key) && views.get(tab.key)}
          <AnnotationGutter
            view={views.get(tab.key)!}
            annotation={annotations.visible(tab.key)!}
            close={() => annotations.close(tab.key)}
            {showDiff}
            annotate={(target, line) => annotateRevision(tab.key, target, line)}
            onerror={(error) => (actionError = `${CANNOT_OPEN}: ${errorText(error)}`)}
          />
        {/if}
      {:else if tab.content?.kind === "binary"}
        <p class="empty">{binaryText(tab.path, tab.content.size, tab.content.tooLarge)}</p>
      {:else if tab.content?.kind === "absent"}
        <p class="empty">{absentText(tab)}</p>
      {/if}
    </section>
  {/each}
</main>

{#if menu}
  <Menu x={menu.x} y={menu.y} label={menu.label} items={menu.items} onclose={() => (menu = null)} />
{/if}

{#if actionError}
  <Dialog
    title="Error"
    width={480}
    onclose={() => (actionError = null)}
    onsubmit={() => (actionError = null)}
  >
    <p class="message">{actionError}</p>
    {#snippet footer()}
      <button class="button default" data-autofocus>OK</button>
    {/snippet}
  </Dialog>
{/if}

{#if annotations.failure}
  <Dialog
    title="Annotate"
    width={480}
    onclose={() => (annotations.failure = null)}
    onsubmit={() => (annotations.failure = null)}
  >
    <p class="message">{annotations.failure}</p>
    {#snippet footer()}
      <button class="button default" data-autofocus>OK</button>
    {/snippet}
  </Dialog>
{/if}

{#if failure}
  <Dialog
    title="Could Not Load Content"
    width={480}
    onclose={() => (failure = null)}
    onsubmit={() => (failure = null)}
  >
    <p class="message">{failure}</p>
    {#snippet footer()}
      <button class="button default" data-autofocus>OK</button>
    {/snippet}
  </Dialog>
{/if}

<style>
  main {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--editor);
  }

  .tabs {
    display: flex;
    flex: none;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .tabs.rows {
    flex-wrap: wrap;
  }

  .tab {
    display: flex;
    gap: 6px;
    align-items: center;
    height: 30px;
    box-sizing: border-box;
    padding: 0 6px 0 10px;
    color: var(--text);
    white-space: nowrap;
    border-bottom: 2px solid transparent;
    cursor: default;
  }

  .tab:hover {
    background: var(--hover);
  }

  .tab.active {
    border-bottom-color: var(--accent);
  }

  .file-icon {
    position: relative;
    display: inline-flex;
  }

  /* The padlock is a layer of the file's icon, as `LayeredIcon` draws it. */
  .lock {
    position: absolute;
    inset: 0;
    display: inline-flex;
  }

  .close {
    display: inline-flex;
    padding: 2px;
    background: none;
    border: 0;
    border-radius: 4px;
  }

  .close:hover {
    background: var(--pressed);
  }

  .file {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .file.hidden {
    display: none;
  }

  .host {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .host :global(.cm-editor) {
    height: 100%;
  }

  .notice {
    flex: none;
    padding: 6px 10px;
    margin: 0;
    color: var(--text);
    /* `EditorNotificationPanel.Status.Warning`. */
    background: var(--notice-warning);
    border-bottom: 1px solid var(--notice-warning-border);
  }

  /* `EditorNotificationPanel.Status.Error` (`LightColors.RED`). */
  .notice.error {
    display: flex;
    gap: 12px;
    align-items: baseline;
    background: var(--notice-error);
    border-bottom-color: var(--notice-error-border);
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    white-space: nowrap;
    background: none;
    border: 0;
  }

  .empty {
    margin: auto;
    color: var(--text-secondary);
  }

  .message {
    margin: 0;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
</style>
