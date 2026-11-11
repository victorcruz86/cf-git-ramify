<script lang="ts">
  import Dialog from "../lib/Dialog.svelte";
  import type { OpenAnswer } from "./open";

  // `ProjectUtil.confirmOpenNewProject`: where a project goes when this window already has one.
  // This Window is the default button; Escape is Cancel. `Don't ask again` saves the button
  // chosen as `Open project in`, and does nothing with Cancel.
  interface Props {
    name: string;
    onanswer: (answer: OpenAnswer) => void;
  }

  let { name, onanswer }: Props = $props();

  const DONT_ASK = "Don't ask again";
  let remember = $state(false);

  const answer = (choice: OpenAnswer["choice"]) => onanswer({ choice, remember });
</script>

<Dialog title="Open Project" width={420} onclose={() => answer("cancel")} onsubmit={() => answer("here")}>
  <p>Where would you like to open the project '{name}'?</p>
  <label class="checkbox-label">
    <input type="checkbox" aria-label={DONT_ASK} bind:checked={remember} />
    <span class="check" data-state={remember ? "on" : "off"}></span>
    <span>{DONT_ASK}</span>
  </label>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => answer("cancel")}>Cancel</button>
    <button type="button" class="button" onclick={() => answer("new")}>New Window</button>
    <button class="button default" data-autofocus>This Window</button>
  {/snippet}
</Dialog>

<style>
  p {
    margin: 0 0 10px;
    overflow-wrap: anywhere;
  }
</style>
