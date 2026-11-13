<script lang="ts">
  import { onMount } from "svelte";
  import { Channel } from "@tauri-apps/api/core";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { terminalClose, terminalOpen, terminalResize, terminalWrite } from "../lib/api";
  import { errorText } from "../lib/errors";
  import { TERMINAL_OPTIONS } from "./theme";

  interface Props {
    // It gets the keyboard when its tab is shown.
    focused: boolean;
    visible: boolean;
    // More than one pane in the tab: the focused one is outlined.
    outlined: boolean;
    onfocus: () => void;
    onsession: (id: number) => void;
    oncontextmenu: (event: MouseEvent) => void;
  }

  let { focused, visible, outlined, onfocus, onsession, oncontextmenu }: Props = $props();

  let host: HTMLDivElement;
  let terminal = $state.raw<Terminal | null>(null);

  onMount(() => {
    const term = new Terminal(TERMINAL_OPTIONS);
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    terminal = term;
    // A hidden tab has no size: fitting it would leave the terminal one column wide.
    const fitNow = () => {
      if (host.clientWidth > 0 && host.clientHeight > 0) fit.fit();
    };
    fitNow();
    const observer = new ResizeObserver(fitNow);
    observer.observe(host);

    const output = new Channel<ArrayBuffer | number[]>();
    output.onmessage = (bytes) =>
      term.write(bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : Uint8Array.from(bytes));
    let session: number | null = null;
    let gone = false;
    terminalOpen(term.cols, term.rows, output)
      .then((id) => {
        if (gone) {
          void terminalClose(id);
          return;
        }
        session = id;
        onsession(id);
        term.onData((data) => void terminalWrite(id, data).catch(() => {}));
        term.onResize(({ cols, rows }) => void terminalResize(id, cols, rows).catch(() => {}));
        // The pane may have been fitted while the session started.
        void terminalResize(id, term.cols, term.rows).catch(() => {});
      })
      .catch((failure) => term.write(`\r\n${errorText(failure)}\r\n`));
    term.textarea?.addEventListener("focus", onfocus);

    return () => {
      gone = true;
      observer.disconnect();
      if (session !== null) void terminalClose(session).catch(() => {});
      term.dispose();
    };
  });

  $effect(() => {
    if (focused && visible) terminal?.focus();
  });

  // What is typed belongs to the shell, not to the app's shortcuts. ⌘ combinations still reach
  // the app, as IntelliJ's terminal leaves them to the IDE.
  function keydown(event: KeyboardEvent) {
    if (!event.metaKey) event.stopPropagation();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="pane"
  class:outlined={outlined && focused}
  bind:this={host}
  onkeydown={keydown}
  onmousedown={onfocus}
  oncontextmenu={(event) => {
    event.preventDefault();
    oncontextmenu(event);
  }}
></div>

<style>
  .pane {
    position: absolute;
    inset: 0;
    padding: 4px 0 0 8px;
    background: var(--editor);
    box-sizing: border-box;
  }

  .pane.outlined {
    box-shadow: inset 0 0 0 1px var(--focus);
  }

  .pane :global(.xterm) {
    height: 100%;
  }
</style>
