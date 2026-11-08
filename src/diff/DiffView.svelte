<script lang="ts">
  import { MergeView } from "@codemirror/merge";
  import { EditorState, type Extension } from "@codemirror/state";
  import { EditorView, lineNumbers } from "@codemirror/view";
  import { onDestroy } from "svelte";
  import { fileSides, type ApiError, type ChangedFile } from "../lib/api";
  import Menu, { type MenuItem } from "../lib/Menu.svelte";
  import { setWrap, softWrap, wrapping } from "./editor";
  import { languageFor } from "./language";
  import { inGutter, loadWrap, softWrapItem, storeWrap } from "./options";
  import { sideMessage, sideText, unchangedText } from "./sides";
  import { diffTheme } from "./theme";

  interface Props {
    hash: string;
    file: ChangedFile;
    // What it is compared against. Without it, the old side does not exist.
    base: string | null;
    onopened?: () => void;
  }

  let { hash, file, base, onopened }: Props = $props();

  let notice = $state<string | null>(null);
  let host: HTMLDivElement;
  let view: MergeView | null = null;
  let request = 0;
  let menu = $state.raw<{ x: number; y: number; items: MenuItem[] } | null>(null);
  // `Soft-Wrap` of the place `VcsLogView`, the diff docked in the log and in the push dialog.
  let wrap = loadWrap("VcsLogView");

  function clearView() {
    view?.destroy();
    view = null;
  }

  function showError(failure: unknown) {
    notice = (failure as ApiError).message ?? String(failure);
  }

  async function open(commit: string, target: ChangedFile, parent: string | null) {
    const token = ++request;
    notice = null;
    try {
      const sides = await fileSides(commit, target, parent);
      if (token !== request) return;
      clearView();
      notice =
        sideMessage(sides.old) ??
        sideMessage(sides.new) ??
        unchangedText(sides, "the first parent");
      if (!notice) {
        view = createView(sideText(sides.old), sideText(sides.new), target.path);
      }
      onopened?.();
    } catch (failure) {
      if (token === request) showError(failure);
    }
  }

  function createView(oldText: string, newText: string, path: string): MergeView {
    const extensions: Extension[] = [
      lineNumbers(),
      diffTheme,
      EditorState.readOnly.of(true),
      EditorView.editable.of(false),
      wrapping.of(softWrap(wrap)),
      languageFor(path),
    ];
    return new MergeView({
      a: { doc: oldText, extensions },
      b: { doc: newText, extensions },
      parent: host,
      collapseUnchanged: { margin: 3, minSize: 8 },
    });
  }

  // Both sides wrap together, without being rebuilt; MergeView measures its own spacers.
  function toggleWrap(on: boolean) {
    wrap = on;
    storeWrap("VcsLogView", on);
    if (view) setWrap([view.a, view.b], on);
  }

  // The gutter popup of SetEditorSettingsActionGroup. The text has no menu here.
  function contextmenu(event: MouseEvent) {
    if (!view || !inGutter(event)) return;
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY, items: [softWrapItem(wrap, toggleWrap)] };
  }

  $effect(() => {
    open(hash, file, base);
  });

  onDestroy(clearView);
</script>

<div class="diff">
  {#if notice}
    <p>{notice}</p>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="host" bind:this={host} oncontextmenu={contextmenu}></div>
</div>

{#if menu}
  <Menu x={menu.x} y={menu.y} label="Diff" items={menu.items} onclose={() => (menu = null)} />
{/if}

<style>
  .diff {
    position: relative;
    height: 100%;
    overflow: auto;
  }

  p {
    padding: 16px;
    margin: 0;
    color: var(--text-secondary);
  }
</style>
