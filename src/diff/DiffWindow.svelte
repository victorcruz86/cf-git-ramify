<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    diffFlushed,
    diffWindowOpen,
    diffWindowOpenRequest,
    diffWindowRequest,
    effectiveStaging,
    emitDiffDrawn,
    emitDiffReady,
    emitSelectionChanged,
    hideWindow,
    LOCAL_REVISION,
    onDiffClose,
    onDiffFlush,
    onComparison,
    onDiffOpen,
    onDiffRequest,
    onRepoChanged,
    onSelectionLost,
    onSelectionState,
    onSettingsChanged,
    perfEnabled,
    repoStatus,
    settingsGet,
    showWindow,
    type Changelists,
    type Comparison,
    type DiffRequest,
    type DiffTiming,
    type FileChange,
    type LineSelection,
    type RevisionPair,
    type SelectionChange,
    type SideRef,
    type SelectionOwner,
    type StageComparison,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { matchDiffShortcut } from "../lib/keymap";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import type { IgnorePolicy } from "./blocks";
  import type { HighlightMode } from "./editor";
  import FilePane, { type NoticeKind } from "./FilePane.svelte";
  import ThreeVersionsPane from "./ThreeVersionsPane.svelte";
  import RevisionsPreview from "./RevisionsPreview.svelte";
  import {
    loadOptions,
    softWrapItem,
    storeOptions,
    storeWrap,
    type DiffOptions,
  } from "./options";
  import { frameLimit, nextFrame } from "../perf/run";

  interface Notice {
    id: number;
    kind: NoticeKind;
    text: string;
    action?: { label: string; run: () => void };
  }

  const NOTICE_MS = 10_000;
  const SAVE_PASSES = 5;
  const MENU_FILES = 200;
  const COMBINED_MARGIN = "800px";
  const COMBINED_ESTIMATE = 240;

  const VIEWERS = [
    { value: false, label: "Side-by-side viewer" },
    { value: true, label: "Unified viewer" },
  ];
  const IGNORE: { value: IgnorePolicy; label: string }[] = [
    { value: "none", label: "Do not ignore" },
    { value: "trim", label: "Trim whitespaces" },
    { value: "whitespaces", label: "Ignore whitespaces" },
    { value: "emptyLines", label: "Ignore whitespaces and empty lines" },
  ];
  const COMPARISONS: { value: StageComparison; label: string }[] = [
    { value: "staged", label: "HEAD and Staged" },
    { value: "local", label: "Staged and Local" },
    { value: "three", label: "Compare HEAD, Staged and Local Versions" },
  ];
  const HIGHLIGHT: { value: HighlightMode; label: string }[] = [
    { value: "lines", label: "Highlight lines" },
    { value: "words", label: "Highlight words" },
    { value: "none", label: "Do not highlight" },
  ];

  let files = $state.raw<FileChange[]>([]);
  // A request of two revisions (H§4): its own files and titles, no status, stage or line selection.
  let revisions = $state.raw<{ files: RevisionPair[]; title: string } | null>(null);
  // Each request of two revisions builds its panes again, even for the same file.
  let requestCount = $state(0);
  // The path of the working-tree file, or the index of the pair of a request of two revisions.
  let current = $state<string | null>(null);
  let version = $state(0);
  // Until the application says who keeps them, nobody does: no line checkboxes.
  let owner = $state<SelectionOwner>("none");
  let included = $state.raw(new Set<string>());
  let selections = $state.raw(new Map<string, LineSelection[]>());
  let lists = $state.raw<Changelists | null>(null);
  let stagingArea = $state(false);
  let chosenComparison = $state<StageComparison>("local");
  // Files without the three versions: they are compared two at a time, like ThreeSidesProducer, and
  // each one with the comparison chosen when switching to two: after that it only changes through
  // `transition`.
  let threeless = $state.raw(new Map<string, Comparison>());
  let options = $state.raw<DiffOptions>(loadOptions());
  let combined = $state(false);
  let notices = $state.raw<Notice[]>([]);
  let nextNotice = 0;
  let menu = $state.raw<{ x: number; y: number; label: string; items: MenuItem[] } | null>(null);
  let single = $state<FilePane>();
  let three = $state<ThreeVersionsPane>();
  const panes = $state<Record<string, FilePane | undefined>>({});
  let near = $state.raw(new Set<string>());
  const heights = new Map<string, number>();
  let scroller = $state<HTMLElement>();
  let observer: IntersectionObserver | null = null;
  let transitions: Promise<unknown> = Promise.resolve();
  let measuring = $state(false);

  // What the toolbar steps through: the changed files, or the pairs of the request.
  const entries = $derived<{ key: string; label: string }[]>(
    revisions
      ? revisions.files.map((pair, index) => ({ key: String(index), label: pairLabel(pair) }))
      : files.map((file) => ({ key: file.path, label: file.path })),
  );
  const position = $derived(entries.findIndex((entry) => entry.key === current));
  const currentPair = $derived(
    revisions && current !== null ? (revisions.files[Number(current)] ?? null) : null,
  );
  const comparison = $derived(stagingArea && !revisions ? chosenComparison : null);
  // The combined view has two panes: with the three versions it is not offered.
  const showingThree = $derived(
    comparison === "three" && !combined && current !== null && !threeless.has(current),
  );
  const activePane = $derived(
    combined ? (current ? panes[current] : undefined) : showingThree ? undefined : single,
  );
  const navigable = $derived(showingThree ? three : activePane);

  function notify(kind: NoticeKind, text: string, action?: Notice["action"]) {
    if (!action && notices.some((notice) => notice.text === text)) return;
    const id = nextNotice++;
    notices = [...notices.filter((notice) => notice.text !== text), { id, kind, text, action }];
    if (kind === "info" && !action) setTimeout(() => dismiss(id), NOTICE_MS);
  }

  function dismiss(id: number) {
    notices = notices.filter((notice) => notice.id !== id);
  }

  function setOption(change: Partial<DiffOptions>) {
    options = { ...options, ...change };
    storeOptions(options);
  }

  // The diff window is the place `Default`.
  function setWrap(wrap: boolean) {
    options = { ...options, wrap };
    storeWrap("Default", wrap);
  }

  const byPath = (left: FileChange, right: FileChange) => (left.path < right.path ? -1 : 1);

  // The file the pane saves: the local side, else the right one.
  const localPath = (pair: RevisionPair) =>
    pair.left.revision === LOCAL_REVISION ? pair.left.path : pair.right.path;

  function pairLabel(pair: RevisionPair): string {
    const { left, right } = pair;
    return left.revision === null || left.path === right.path
      ? right.path
      : `${left.path} -> ${right.path}`;
  }

  async function refreshFiles() {
    try {
      const status = await repoStatus(false);
      const next = status.filter((file) => file.state !== "ignored").sort(byPath);
      const staying = new Set(next.map((file) => file.path));
      const leaving = combined
        ? files.filter((file) => !staying.has(file.path) && panes[file.path]?.isDirty())
        : [];
      files = leaving.length === 0 || (await transition()) ? next : [...next, ...leaving].sort(byPath);
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  // It is repeated because while one file is being written, another can still be typed into. With a
  // limit: whoever never stops typing must not leave the transition waiting forever.
  async function saveAll(): Promise<boolean> {
    for (let pass = 0; pass < SAVE_PASSES; pass++) {
      const mounted = combined ? Object.values(panes) : [single, three];
      for (const pane of mounted) {
        if (pane && !(await pane.flush())) return false;
      }
      if (!mounted.some((pane) => pane?.isDirty())) return true;
    }
    notify("error", "The text keeps changing while it is being saved: nothing else was done");
    return false;
  }

  // Removing an editor would lose what it has unsaved. Everything that removes one goes through
  // here: it is saved as IntelliJ does, and if that is not possible the change does not happen and
  // the error stays in view.
  function transition(change: () => void = () => {}): Promise<boolean> {
    const step = transitions.then(async () => {
      if (!(await saveAll())) return false;
      change();
      return true;
    });
    transitions = step.catch(() => false);
    return step;
  }

  function refresh() {
    version++;
    if (!revisions) refreshFiles();
  }

  // The selection is kept by its owner (the Commit view or the dialog), and this window shows it as soon
  // as it changes, without waiting for the notice to come back.
  function selectionChanged(change: SelectionChange) {
    const nextSelections = new Map(selections);
    if (change.selection.length > 0) nextSelections.set(change.path, change.selection);
    else nextSelections.delete(change.path);
    selections = nextSelections;
    const nextIncluded = new Set(included);
    if (change.included) nextIncluded.add(change.path);
    else nextIncluded.delete(change.path);
    included = nextIncluded;
    emitSelectionChanged(change);
  }

  async function open(path: string) {
    if (path === current) return;
    // The pairs of a request are all here: only the file shown changes.
    if (revisions) {
      transition(() => (current = path));
      return;
    }
    try {
      await diffWindowOpen(path);
    } catch (failure) {
      notify("error", errorText(failure));
    }
  }

  // If the file does not change, the title and the backend path go back to the one that is still
  // open.
  async function showFile(path: string) {
    if (path === current) return;
    const changed = await transition(() => {
      current = path;
      threeless = new Map();
      if (combined) document.getElementById(sectionId(path))?.scrollIntoView();
    });
    if (!changed && current) {
      await diffWindowOpen(current).catch((failure) => notify("error", errorText(failure)));
    }
  }

  // Another request arrives with the window open: what was typed is saved first, as when switching
  // files, and if that is not possible the window and the backend keep the one it had.
  async function showRequest(request: DiffRequest) {
    // The request already shown: what the backend sends back when the window restores it after a
    // save that failed. Showing it again would retry that save and restore it again, forever.
    if (request.kind === "revisions" && revisions && sameRequest(request, revisions)) {
      // The same files asked for again from another one of them: only the file shown changes.
      if (request.selected !== undefined) void open(openedOn(request));
      return;
    }
    const previous: DiffRequest | null = revisions
      ? { kind: "revisions", ...revisions }
      : current !== null
        ? { kind: "worktree", path: current }
        : null;
    if (request.kind === "worktree") {
      if (!revisions) {
        await showFile(request.path);
        return;
      }
      if (await transition(() => leaveRevisions(request.path))) {
        await refreshFiles();
        return;
      }
    } else if (
      await transition(() => {
        if (combined) setCombined(false);
        revisions = { files: request.files, title: request.title };
        requestCount++;
        current = openedOn(request);
        threeless = new Map();
      })
    ) {
      return;
    }
    if (previous) {
      await diffWindowOpenRequest(previous).catch((failure) => notify("error", errorText(failure)));
    }
  }

  const sameSide = (left: SideRef, right: SideRef) =>
    left.revision === right.revision && left.path === right.path && left.title === right.title;

  // The pair a request of two revisions opens on (`ListSelection`'s index), the first without one.
  const openedOn = ({ files, selected }: { files: RevisionPair[]; selected?: number }) =>
    String(Math.max(0, Math.min(selected ?? 0, files.length - 1)));

  const sameRequest = (
    request: { files: RevisionPair[]; title: string },
    shown: { files: RevisionPair[]; title: string },
  ) =>
    request.title === shown.title &&
    request.files.length === shown.files.length &&
    request.files.every(
      (pair, index) =>
        sameSide(pair.left, shown.files[index].left) &&
        sameSide(pair.right, shown.files[index].right) &&
        (pair.center === undefined) === (shown.files[index].center === undefined) &&
        (!pair.center || sameSide(pair.center, shown.files[index].center!)),
    );

  function leaveRevisions(path: string) {
    revisions = null;
    current = path;
    threeless = new Map();
  }

  async function drawn(timing: Omit<DiffTiming, "paintMs">) {
    await emitDiffDrawn({ ...timing, paintMs: await nextFrame(frameLimit()) });
  }

  async function closeWindow() {
    if (await transition()) await hideWindow();
  }

  function step(direction: 1 | -1) {
    const next = entries[position + direction];
    if (next) open(next.key);
  }

  // When reaching the last change of a file, F7 continues with the next one.
  async function navigate(direction: 1 | -1) {
    if (navigable?.navigate(direction)) return;
    if (!combined) return;
    const next = files[position + direction];
    if (!next) return;
    current = next.path;
    document.getElementById(sectionId(next.path))?.scrollIntoView();
    await tick();
    panes[next.path]?.navigate(direction);
  }

  // What is shown changes: as when switching files, what was typed is saved first.
  function setComparison(next: StageComparison) {
    if (next === chosenComparison) return;
    transition(() => {
      chosenComparison = next;
      threeless = new Map();
      if (next === "three" && combined) setCombined(false);
    });
  }

  // With the three versions, the file that does not have them is compared as from its group.
  function comparisonFor(path: string): Comparison | null {
    if (comparison !== "three") return comparison;
    return threeless.get(path) ?? "local";
  }

  // Removes the three panes: like every view change, it goes through `transition`.
  function fallBack(path: string) {
    transition(() => {
      const file = files.find((change) => change.path === path);
      threeless = new Map([...threeless, [path, file && !file.unstaged ? "staged" : "local"]]);
    });
  }

  function setStagingArea(next: boolean) {
    if (next !== stagingArea) transition(() => (stagingArea = next));
  }

  function keydown(event: KeyboardEvent) {
    if (menu) return;
    const action = matchDiffShortcut(event);
    if (!action) return;
    event.preventDefault();
    if (action === "NextDiff") navigate(1);
    else if (action === "PreviousDiff") navigate(-1);
    else if (action === "Diff.NextChange") step(1);
    else if (action === "Diff.PrevChange") step(-1);
    else if (action === "SaveAll") transition();
    else if (action === "Vcs.RollbackChangedLines") activePane?.rollbackLines();
    else if (action === "Vcs.MoveChangedLinesToChangelist") activePane?.moveLines();
    else if (action === "CloseDiff") closeWindow();
  }

  function openMenu<T>(
    event: MouseEvent,
    label: string,
    choices: { value: T; label: string }[],
    selected: T,
    choose: (value: T) => void,
  ) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      label,
      items: choices.map((choice) => ({
        label: choice.label,
        checked: choice.value === selected,
        run: () => choose(choice.value),
      })),
    };
  }

  function openFilesMenu(event: MouseEvent) {
    const first = Math.max(0, position - MENU_FILES / 2);
    const nearby = entries.slice(first, first + MENU_FILES);
    openMenu(
      event,
      "Changed files",
      nearby.map((entry) => ({ value: entry.key, label: entry.label })),
      current,
      (key) => key && open(key),
    );
  }

  function openSettings(event: MouseEvent) {
    const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: bounds.left,
      y: bounds.bottom + 2,
      label: "Settings",
      items: [
        softWrapItem(options.wrap, setWrap),
        {
          label: "Align Changes Highlighting",
          checked: options.align,
          run: () => setOption({ align: !options.align }),
        },
      ],
    };
  }

  const labelOf = <T,>(choices: { value: T; label: string }[], value: T) =>
    choices.find((choice) => choice.value === value)?.label ?? "";

  const sectionId = (path: string) => `file-${encodeURIComponent(path)}`;

  // In the combined view only the editors near what is visible exist. A file with unsaved changes
  // stays, so they are not lost.
  function watchSection(section: HTMLElement, path: string) {
    observer?.observe(section);
    section.dataset.path = path;
    return { destroy: () => observer?.unobserve(section) };
  }

  function sectionsVisible(entries: IntersectionObserverEntry[]) {
    const next = new Set(near);
    for (const entry of entries) {
      const path = (entry.target as HTMLElement).dataset.path ?? "";
      if (entry.isIntersecting) {
        next.add(path);
      } else if (!panes[path]?.isDirty()) {
        heights.set(path, entry.boundingClientRect.height);
        next.delete(path);
      }
    }
    near = next;
  }

  function setCombined(next: boolean) {
    combined = next;
    near = new Set();
    observer?.disconnect();
    observer = null;
    if (next && scroller) {
      observer = new IntersectionObserver(sectionsVisible, {
        root: scroller,
        rootMargin: COMBINED_MARGIN,
      });
    }
    if (next && current) {
      tick().then(() => document.getElementById(sectionId(current ?? ""))?.scrollIntoView());
    }
  }

  onMount(() => {
    const stops = [
      onRepoChanged(refresh),
      onDiffOpen((path) => showRequest({ kind: "worktree", path })),
      onDiffRequest(showRequest),
      onDiffClose(closeWindow),
      onDiffFlush(async () => {
        await diffFlushed(await transition());
      }),
      onSelectionState((state) => {
        owner = state.owner;
        included = new Set(state.included);
        selections = new Map(Object.entries(state.selections));
        lists = state.lists;
      }),
      onSelectionLost((lost) => notify("warning", lost.text)),
      onComparison(setComparison),
      onSettingsChanged((settings) => setStagingArea(effectiveStaging(settings))),
    ];
    (async () => {
      try {
        stagingArea = effectiveStaging(await settingsGet());
        measuring = await perfEnabled();
        const initial = await diffWindowRequest();
        if (initial?.kind === "revisions") {
          revisions = { files: initial.files, title: initial.title };
          current = openedOn(initial);
        } else {
          current = initial?.path ?? null;
          await refreshFiles();
        }
        await Promise.all(stops);
        await emitDiffReady();
      } catch (failure) {
        notify("error", errorText(failure));
      } finally {
        await showWindow();
      }
    })();
    return () => {
      observer?.disconnect();
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });
</script>

<!-- Like IntelliJ when it stops being the active window. If it is not possible, the error is already
     in view. -->
<svelte:window onkeydown={keydown} onblur={() => transition()} />

<main>
  <div class="toolbar" role="toolbar" aria-label="Diff">
    <button
      type="button"
      class="action"
      title="Previous Difference (⇧F7)"
      aria-label="Previous Difference"
      onclick={() => navigate(-1)}
    >
      <Icon name="up" />
    </button>
    <button
      type="button"
      class="action"
      title="Next Difference (F7)"
      aria-label="Next Difference"
      onclick={() => navigate(1)}
    >
      <Icon name="down" />
    </button>
    <button
      type="button"
      class="action"
      title="Compare Previous File (⌃⇧←)"
      aria-label="Compare Previous File"
      disabled={position <= 0}
      onclick={() => step(-1)}
    >
      <Icon name="left" />
    </button>
    <button
      type="button"
      class="action"
      title="Compare Next File (⌃⇧→)"
      aria-label="Compare Next File"
      disabled={position < 0 || position >= entries.length - 1}
      onclick={() => step(1)}
    >
      <Icon name="right" />
    </button>
    <button
      type="button"
      class="choice file"
      onclick={openFilesMenu}
      disabled={entries.length === 0}
    >
      <span class="path">{entries[position]?.label ?? current ?? ""}</span>
      {#if position >= 0}
        <span class="count">{position + 1}/{entries.length}</span>
      {/if}
      <Icon name="chevronDown" size={12} />
    </button>
    <span class="toolbar-separator"></span>
    {#if comparison}
      <button
        type="button"
        class="choice"
        onclick={(event) => openMenu(event, "Compare", COMPARISONS, comparison, setComparison)}
      >
        {labelOf(COMPARISONS, comparison)}
        <Icon name="chevronDown" size={12} />
      </button>
    {/if}
    <button
      type="button"
      class="choice"
      onclick={(event) =>
        openMenu(event, "Viewer", VIEWERS, options.unified, (unified) => setOption({ unified }))}
    >
      {labelOf(VIEWERS, options.unified)}
      <Icon name="chevronDown" size={12} />
    </button>
    <button
      type="button"
      class="choice"
      onclick={(event) =>
        openMenu(event, "Ignore Changes", IGNORE, options.ignore, (ignore) => setOption({ ignore }))}
    >
      {labelOf(IGNORE, options.ignore)}
      <Icon name="chevronDown" size={12} />
    </button>
    <button
      type="button"
      class="choice"
      onclick={(event) =>
        openMenu(event, "Highlighting Mode", HIGHLIGHT, options.highlight, (highlight) =>
          setOption({ highlight }),
        )}
    >
      {labelOf(HIGHLIGHT, options.highlight)}
      <Icon name="chevronDown" size={12} />
    </button>
    <button
      type="button"
      class="action"
      class:toggled={options.collapse}
      title="Collapse Unchanged Fragments"
      aria-label="Collapse Unchanged Fragments"
      aria-pressed={options.collapse}
      onclick={() => setOption({ collapse: !options.collapse })}
    >
      <Icon name="collapseAll" />
    </button>
    <button
      type="button"
      class="action"
      class:toggled={options.sync}
      title="Synchronize Scrolling"
      aria-label="Synchronize Scrolling"
      aria-pressed={options.sync}
      onclick={() => setOption({ sync: !options.sync })}
    >
      <Icon name="synchronizeScrolling" />
    </button>
    <button
      type="button"
      class="action"
      title="Settings"
      aria-label="Settings"
      onclick={openSettings}
    >
      <Icon name="settings" />
    </button>
    <button
      type="button"
      class="action"
      class:toggled={combined}
      title="Show Combined Diff"
      aria-label="Show Combined Diff"
      aria-pressed={combined}
      disabled={comparison === "three" || revisions !== null}
      onclick={() => transition(() => setCombined(!combined))}
    >
      <Icon name="diff" />
    </button>
  </div>

  <div class="content" class:combined bind:this={scroller}>
    {#if combined}
      {#each files as file (file.path)}
        <section id={sectionId(file.path)} use:watchSection={file.path}>
          <header class:current={file.path === current}>{file.path}</header>
          {#if near.has(file.path)}
            {@const fileComparison = comparisonFor(file.path)}
            <FilePane
              bind:this={panes[file.path]}
              path={file.path}
              embedded
              {version}
              {options}
              comparison={fileComparison}
              checkboxes={fileComparison !== null || owner !== "none"}
              included={fileComparison ? fileComparison === "staged" : included.has(file.path)}
              selection={fileComparison ? [] : (selections.get(file.path) ?? [])}
              lists={fileComparison ? null : lists}
              onselection={selectionChanged}
              onnotice={notify}
              onwrap={setWrap}
              onactive={() => (current = file.path)}
            />
          {:else}
            <div style:height="{heights.get(file.path) ?? COMBINED_ESTIMATE}px"></div>
          {/if}
        </section>
      {/each}
    {:else if currentPair?.center}
      {#key `${requestCount}/${current}`}
        <RevisionsPreview pair={currentPair} place="Default" />
      {/key}
    {:else if currentPair}
      {#key `${requestCount}/${current}`}
        <FilePane
          bind:this={single}
          path={localPath(currentPair)}
          pair={currentPair}
          {version}
          {options}
          checkboxes={false}
          included={false}
          selection={[]}
          lists={null}
          onselection={() => {}}
          onnotice={notify}
          onwrap={setWrap}
          ondrawn={measuring ? drawn : undefined}
        />
      {/key}
    {:else if current}
      {#key current}
        {@const path = current}
        {@const fileComparison = comparisonFor(path)}
        {#if showingThree}
          <ThreeVersionsPane
            bind:this={three}
            {path}
            oldPath={files.find((file) => file.path === path)?.oldPath ?? null}
            {version}
            {options}
            onnotice={notify}
            onwrap={setWrap}
            onfallback={() => fallBack(path)}
          />
        {:else}
          <FilePane
            bind:this={single}
            {path}
            {version}
            {options}
            comparison={fileComparison}
            checkboxes={fileComparison !== null || owner !== "none"}
            included={fileComparison ? fileComparison === "staged" : included.has(path)}
            selection={fileComparison ? [] : (selections.get(path) ?? [])}
            lists={fileComparison ? null : lists}
            onselection={selectionChanged}
            onnotice={notify}
            onwrap={setWrap}
            ondrawn={measuring ? drawn : undefined}
          />
        {/if}
      {/key}
    {/if}
  </div>

  {#if notices.length > 0}
    <div class="notices">
      {#each notices as notice (notice.id)}
        <div class="notice" role={notice.kind === "error" ? "alert" : "status"}>
          <Icon
            name={notice.kind === "error"
              ? "error"
              : notice.kind === "warning"
                ? "warning"
                : "info"}
          />
          <p>{notice.text}</p>
          {#if notice.action}
            {@const action = notice.action}
            <button
              type="button"
              class="link"
              onclick={() => {
                dismiss(notice.id);
                action.run();
              }}
            >
              {action.label}
            </button>
          {/if}
          <button
            type="button"
            class="action close"
            aria-label="Close Notification"
            onclick={() => dismiss(notice.id)}
          >
            <Icon name="close" size={12} />
          </button>
        </div>
      {/each}
    </div>
  {/if}
</main>

{#if menu}
  <Menu
    x={menu.x}
    y={menu.y}
    label={menu.label}
    items={menu.items}
    onclose={() => (menu = null)}
  />
{/if}

<style>
  main {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--editor);
  }

  .toolbar {
    display: flex;
    flex: none;
    gap: 2px;
    align-items: center;
    height: 40px;
    box-sizing: border-box;
    padding: 0 8px;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }

  .action.toggled {
    background: var(--stripe-selected);
  }

  .action.toggled :global(.icon) {
    color: var(--accent-text);
  }

  .choice {
    display: inline-flex;
    gap: 4px;
    align-items: center;
    height: 26px;
    padding: 0 6px;
    font: inherit;
    color: var(--text);
    white-space: nowrap;
    background: none;
    border: 0;
    border-radius: 6px;
  }

  .choice:hover:not(:disabled) {
    background: var(--hover);
  }

  .file {
    flex: 0 1 auto;
    min-width: 80px;
    overflow: hidden;
  }

  .path {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .count {
    color: var(--muted);
  }

  .content {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }

  .content.combined {
    display: block;
    overflow: auto;
  }

  section header {
    position: sticky;
    top: 0;
    z-index: 3;
    padding: 4px 12px;
    color: var(--text);
    background: var(--panel);
    border-top: 1px solid var(--divider);
  }

  section header.current {
    color: var(--text-strong);
  }

  .notices {
    position: absolute;
    right: 12px;
    bottom: 12px;
    z-index: 10;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(420px, calc(100% - 24px));
  }

  .notice {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    padding: 10px 6px 10px 12px;
    color: var(--notification-text);
    background: var(--notification-background);
    border: 1px solid var(--notification-border);
    border-radius: 8px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 35%);
  }

  .notice p {
    flex: 1;
    min-width: 0;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    white-space: nowrap;
    background: none;
    border: 0;
  }

  .notice .close {
    width: 20px;
    height: 20px;
  }
</style>
