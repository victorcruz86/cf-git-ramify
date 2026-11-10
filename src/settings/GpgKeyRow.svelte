<script lang="ts">
  import { onMount } from "svelte";
  import { gpgConfig, gpgKeys, onRepoChanged, type ApiError } from "../lib/api";
  import { errorText } from "../lib/errors";

  interface Props {
    onconfigure: () => void;
  }

  let { onconfigure }: Props = $props();

  // `GpgSignConfigurableRow`: the status label, empty while it loads.
  let status = $state("");
  let error = $state<string | null>(null);
  let hasRepo = $state(false);
  let run = 0;

  // `Commits are signed with GPG key: {0}`: `<uid> (<id>)` when the key list knows the key, else the id.
  async function describe(key: string): Promise<string> {
    try {
      const found = (await gpgKeys()).find((candidate) => candidate.id === key);
      return found?.userId ? `${found.userId} (${found.id})` : key;
    } catch {
      return key;
    }
  }

  // What arrives for an earlier request is dropped.
  export async function refresh() {
    const current = ++run;
    try {
      const config = await gpgConfig();
      const text =
        config.sign && config.key !== null
          ? `Commits are signed with GPG key: ${await describe(config.key)}`
          : "Commits signing with GPG key is not configured";
      if (current !== run) return;
      hasRepo = true;
      error = null;
      status = text;
    } catch (failure) {
      if (current !== run) return;
      if ((failure as ApiError).kind === "not_a_repository") {
        hasRepo = false;
        error = null;
        status = "No Git roots in the project";
      } else {
        hasRepo = true;
        status = "";
        error = errorText(failure);
      }
    }
  }

  onMount(() => {
    void refresh();
    let unlisten: (() => void) | undefined;
    let closed = false;
    onRepoChanged(() => void refresh()).then(
      (stop) => (closed ? stop() : (unlisten = stop)),
      () => {},
    );
    return () => {
      closed = true;
      unlisten?.();
    };
  });
</script>

<div class="gpg-row">
  <button type="button" class="button" disabled={!hasRepo} onclick={onconfigure}>
    Configure GPG Key…
  </button>
  <span class="status">{status}</span>
</div>
{#if error}
  <p class="field-error" role="alert">{error}</p>
{/if}

<style>
  .gpg-row {
    display: flex;
    gap: 10px;
    align-items: center;
  }

  .status {
    min-width: 0;
    overflow: hidden;
    color: var(--text-secondary);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
