<script lang="ts">
  import { onMount } from "svelte";
  import {
    askpassAnswer,
    onAskpassDismissed,
    onAskpassPrompt,
    type AskpassPrompt,
    type PromptKind,
  } from "./api";
  import Dialog from "./Dialog.svelte";

  // Git asks for the username and, once answered, the password. As in IntelliJ, both are typed in the
  // same dialog. Normally the second question arrives instantly; if git dies before that,
  // the wait ends here.
  const SECOND_PROMPT_MS = 5_000;

  let queue = $state.raw<AskpassPrompt[]>([]);
  let username = $state("");
  let password = $state("");
  let problem = $state<string | null>(null);
  let waitingPrompt = $state(false);
  let waitTimer: ReturnType<typeof setTimeout> | undefined;

  const current = $derived(queue[0] ?? null);

  function title(kind: PromptKind): string {
    switch (kind.type) {
      case "username":
      case "password":
        return `Log In to ${kind.url}`;
      case "passphrase":
        return "SSH Key Passphrase";
      case "sshPassword":
        return "SSH Password Login";
      case "hostKey":
        return "SSH Confirmation";
    }
  }

  // The fields are the only place where what was typed lives: nothing is left once closed.
  function close() {
    clearTimeout(waitTimer);
    waitingPrompt = false;
    username = "";
    password = "";
    problem = null;
    queue = queue.slice(1);
  }

  function reply(answer: string | null) {
    if (current) askpassAnswer(current.id, answer);
    close();
  }

  function cancel() {
    if (waitingPrompt) close();
    else reply(null);
  }

  function logIn() {
    if (!current || waitingPrompt) return;
    const { kind } = current;
    if (kind.type !== "username") {
      if (kind.type === "password" && kind.user && !password) problem = "Password cannot be empty";
      else reply(password);
    } else if (!username.trim()) {
      problem = "Username cannot be empty";
    } else if (!password) {
      problem = "Password cannot be empty";
    } else {
      problem = null;
      askpassAnswer(current.id, username);
      waitingPrompt = true;
      waitTimer = setTimeout(close, SECOND_PROMPT_MS);
    }
  }

  function isSecondPrompt({ kind }: AskpassPrompt): boolean {
    return (
      waitingPrompt &&
      kind.type === "password" &&
      current?.kind.type === "username" &&
      kind.url === current.kind.url &&
      kind.user === username
    );
  }

  function asked(prompt: AskpassPrompt) {
    if (isSecondPrompt(prompt)) {
      askpassAnswer(prompt.id, password);
      close();
    } else {
      queue = [...queue, prompt];
    }
  }

  function dismissed(id: number) {
    if (current?.id === id) close();
    else queue = queue.filter((prompt) => prompt.id !== id);
  }

  onMount(() => {
    const stops = [onAskpassPrompt(asked), onAskpassDismissed(dismissed)];
    return () => {
      clearTimeout(waitTimer);
      stops.forEach((stop) => stop.then((unlisten) => unlisten()));
    };
  });
</script>

{#if current}
  {#key current.id}
    {@const kind = current.kind}
    <Dialog
      title={title(kind)}
      onclose={cancel}
      onsubmit={kind.type === "hostKey" ? undefined : logIn}
    >
      {#if kind.type === "username" || kind.type === "password"}
        <div class="login">
          {#if kind.type === "username" || kind.user}
            <p class="wide">Enter credentials:</p>
            <label for="askpass-username">Username:</label>
            {#if kind.type === "username"}
              <input
                id="askpass-username"
                class="input"
                data-autofocus
                autocomplete="off"
                autocapitalize="off"
                spellcheck="false"
                bind:value={username}
              />
            {:else}
              <input id="askpass-username" class="input" readonly value={kind.user} />
            {/if}
          {/if}
          <label for="askpass-password">Password:</label>
          <input
            id="askpass-password"
            class="input"
            type="password"
            autocomplete="off"
            data-autofocus={kind.type === "password" ? true : undefined}
            bind:value={password}
          />
        </div>
      {:else if kind.type === "hostKey"}
        <!-- Focus stays on No: the dialog appears on its own, and a stray Enter must not accept
             an unknown host. -->
        <p class="question">{kind.text}</p>
      {:else}
        <label class="secret">
          {kind.type === "passphrase"
            ? `Password for the SSH key "${kind.key}":`
            : `Password for user ${kind.user}:`}
          <input
            class="input"
            type="password"
            autocomplete="off"
            data-autofocus
            bind:value={password}
          />
        </label>
      {/if}
      {#if problem}
        <p class="field-error" role="alert">{problem}</p>
      {/if}

      {#snippet footer()}
        {#if kind.type === "hostKey"}
          <button type="button" class="button" onclick={() => reply("no")}>No</button>
          <button type="button" class="button default" onclick={() => reply("yes")}>Yes</button>
        {:else}
          <button type="button" class="button" onclick={cancel}>Cancel</button>
          <button class="button default" disabled={waitingPrompt}>
            {kind.type === "username" || kind.type === "password" ? "Log In" : "OK"}
          </button>
        {/if}
      {/snippet}
    </Dialog>
  {/key}
{/if}

<style>
  .login {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px;
    align-items: center;
  }

  .wide {
    grid-column: 1 / -1;
    margin: 0;
  }

  .question {
    margin: 0;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .secret {
    display: flex;
    flex-direction: column;
    gap: 8px;
    overflow-wrap: anywhere;
  }
</style>
