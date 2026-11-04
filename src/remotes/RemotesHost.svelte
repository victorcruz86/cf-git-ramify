<script lang="ts">
  import type { Remote } from "../lib/api";
  import CloneDialog from "./CloneDialog.svelte";
  import ConfirmDialog from "./ConfirmDialog.svelte";
  import DefineRemoteDialog from "./DefineRemoteDialog.svelte";
  import InitDialog from "./InitDialog.svelte";
  import {
    addRemote,
    cloneFlow,
    initFlow,
    openExclude,
    proposedName,
    remotesApi,
    type Confirmation,
  } from "./model";
  import RemotesDialog from "./RemotesDialog.svelte";

  interface Props {
    // The root of the repository open in this window, if any.
    current: () => string | null;
    // Opens that repository in this window.
    open: (path: string) => Promise<void>;
  }

  let { current, open }: Props = $props();

  type Request =
    | { kind: "manage" }
    | { kind: "clone" }
    | { kind: "init"; initialDirectory: string }
    | { kind: "define"; initialName: string; taken: string[] }
    | ({ kind: "confirm" } & Confirmation);

  // The dialogs of Manage Remotes, Clone, Create Git Repository and Define Remote. They do not live
  // inside the views: what they open stays alive even if the views reload.
  let asked = $state.raw<{ request: Request; settle: (answer: unknown) => void } | null>(null);
  const request = $derived(asked?.request);

  const ask = <T,>(next: Request) =>
    new Promise<T | null>((resolve) => {
      asked?.settle(null);
      asked = { request: next, settle: (answer) => resolve(answer as T | null) };
    });

  function answer(value: unknown) {
    asked?.settle(value);
    asked = null;
  }

  const deps = { current: () => current(), open: (path: string) => open(path) };
  const cloning = cloneFlow(deps);
  const initializing = initFlow(deps, {
    confirm: async (confirmation) => (await ask<boolean>({ kind: "confirm", ...confirmation })) === true,
  });

  export async function manage() {
    await ask({ kind: "manage" });
  }

  export async function clone() {
    const chosen = await ask<{ url: string; directory: string }>({ kind: "clone" });
    if (chosen) await cloning(chosen.url, chosen.directory);
  }

  // Without an open repository there is no folder to propose.
  export async function init() {
    const directory = await ask<string>({ kind: "init", initialDirectory: current() ?? "" });
    if (directory) await initializing(directory);
  }

  // `Define remote` of the push dialog. `true` if it was defined.
  export async function defineRemote(): Promise<boolean> {
    let remotes: Remote[] = [];
    try {
      remotes = await remotesApi.list();
    } catch {
      remotes = [];
    }
    const chosen = await ask<{ name: string; url: string }>({
      kind: "define",
      initialName: proposedName(remotes),
      taken: remotes.map((remote) => remote.name),
    });
    return chosen ? addRemote(remotesApi, chosen.name, chosen.url) : false;
  }

  export const openExcludeFile = () => openExclude();
</script>

{#if request?.kind === "manage"}
  <RemotesDialog onclose={() => answer(null)} />
{:else if request?.kind === "clone"}
  <CloneDialog onanswer={answer} />
{:else if request?.kind === "init"}
  <InitDialog initialDirectory={request.initialDirectory} onanswer={answer} />
{:else if request?.kind === "define"}
  <DefineRemoteDialog
    initialName={request.initialName}
    initialUrl=""
    taken={request.taken}
    test={(url, name) => remotesApi.testUrl(url, name)}
    onanswer={answer}
  />
{:else if request?.kind === "confirm"}
  <ConfirmDialog title={request.title} message={request.message} onanswer={answer} />
{/if}
