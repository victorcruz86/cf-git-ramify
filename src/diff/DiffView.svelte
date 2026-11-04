<script lang="ts">
  import { MergeView } from "@codemirror/merge";
  import { EditorState, type Extension } from "@codemirror/state";
  import { EditorView, lineNumbers } from "@codemirror/view";
  import { onDestroy } from "svelte";
  import { fileSides, type ApiError, type ChangedFile } from "../lib/api";
  import { languageFor } from "./language";
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
      languageFor(path),
    ];
    return new MergeView({
      a: { doc: oldText, extensions },
      b: { doc: newText, extensions },
      parent: host,
      collapseUnchanged: { margin: 3, minSize: 8 },
    });
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
  <div class="host" bind:this={host}></div>
</div>

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
