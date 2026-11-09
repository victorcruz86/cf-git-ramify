<script lang="ts">
  import { untrack } from "svelte";
  import Dialog from "../lib/Dialog.svelte";
  import { errorText } from "../lib/errors";
  import { remoteProblem, type RemoteProblems } from "./model";

  interface Props {
    initialName: string;
    initialUrl: string;
    // The names of the existing remotes.
    taken: string[];
    // `ls-remote` of the URL; rejects if it does not answer.
    test: (url: string, name: string) => Promise<unknown>;
    onanswer: (answer: { name: string; url: string } | null) => void;
  }

  let { initialName, initialUrl, taken, test, onanswer }: Props = $props();

  // Whoever edits starts with a name, which does not count as a duplicate. The fields start from the
  // props once: what is typed afterwards is the user's.
  const original = untrack(() => initialName);
  let name = $state(original);
  let url = $state(untrack(() => initialUrl));
  let problems = $state<RemoteProblems>({});
  let loading = $state(false);

  // `GitDefineRemoteDialog.doOKAction`: first the fields, then `ls-remote`, and only then does it
  // close. What git says if it fails stays under the URL.
  async function submit() {
    if (loading) return;
    problems = remoteProblem(name, url, taken, original);
    if (problems.name || problems.url) return;
    loading = true;
    const answer = { name: name.trim(), url: url.trim() };
    try {
      await test(answer.url, answer.name);
    } catch (failure) {
      problems = { url: errorText(failure) };
      return;
    } finally {
      loading = false;
    }
    onanswer(answer);
  }
</script>

<Dialog title="Define Remote" onclose={() => onanswer(null)} onsubmit={submit} width={460}>
  <div class="field">
    <label for="remote-name">Name:</label>
    <input
      id="remote-name"
      class="input"
      data-autofocus={name === "" || undefined}
      bind:value={name}
      disabled={loading}
      aria-invalid={problems.name ? true : undefined}
      oninput={() => (problems = { ...problems, name: undefined })}
    />
    {#if problems.name}<span class="problem" role="alert">{problems.name}</span>{/if}
  </div>
  <div class="field">
    <label for="remote-url">URL:</label>
    <input
      id="remote-url"
      class="input"
      data-autofocus={name !== "" || undefined}
      bind:value={url}
      disabled={loading}
      aria-invalid={problems.url ? true : undefined}
      oninput={() => (problems = { ...problems, url: undefined })}
    />
    {#if problems.url}<span class="problem" role="alert">{problems.url}</span>{/if}
  </div>
  {#snippet footer()}
    <button type="button" class="button" onclick={() => onanswer(null)}>Cancel</button>
    <button class="button default" disabled={loading}>OK</button>
  {/snippet}
</Dialog>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
  }

  .problem {
    color: var(--error, #f75464);
    white-space: pre-wrap;
    user-select: text;
  }
</style>
