<script lang="ts">
  // `CommitOptionsPanel` of the Commit Changes dialog: the `Git` group (`GitCommitOptionsUi`) and
  // `Commit Checks`, which holds only `Run Git hooks` and only when the repository has hooks
  // (decision 8, `GitSkipHooksCommitHandlerFactory`). `Create extra commit with file movements` needs
  // a movement provider, which rama does not have.
  interface Props {
    author: string;
    amend: boolean;
    amendAvailable: boolean;
    signOff: boolean;
    runHooks: boolean;
    hooks: boolean;
    // `{0} Checks` takes the commit action's name without its mnemonic.
    actionName: string;
    ontoggleamend: () => void;
  }

  let {
    author = $bindable(),
    amend,
    amendAvailable,
    signOff = $bindable(),
    runHooks = $bindable(),
    hooks,
    actionName,
    ontoggleamend,
  }: Props = $props();
</script>

<div class="options" role="group" aria-label="Commit Options">
  <section aria-labelledby="options-git">
    <h3 id="options-git">Git</h3>
    <label class="author">
      Author:
      <input class="input" bind:value={author} spellcheck="false" />
    </label>
    <label
      class="checkbox-label"
      class:disabled={!amendAvailable}
      title="Merge this commit with the previous one"
    >
      <input type="checkbox" checked={amend} disabled={!amendAvailable} onchange={ontoggleamend} />
      <span class="check" data-state={amend ? "on" : "off"}></span>
      Amend commit
    </label>
    <label
      class="checkbox-label"
      title={"Adds the following line at the end of the commit message:\nSigned-off by: "}
    >
      <input type="checkbox" bind:checked={signOff} />
      <span class="check" data-state={signOff ? "on" : "off"}></span>
      Sign-off commit
    </label>
  </section>

  {#if hooks}
    <section aria-labelledby="options-checks">
      <h3 id="options-checks">{actionName} Checks</h3>
      <label
        class="checkbox-label"
        title="If unchecked, Git hooks will be skipped with the '--no-verify' parameter"
      >
        <input type="checkbox" bind:checked={runHooks} />
        <span class="check" data-state={runHooks ? "on" : "off"}></span>
        Run Git hooks
      </label>
    </section>
  {/if}
</div>

<style>
  .options {
    display: flex;
    flex-direction: column;
    gap: 16px;
    box-sizing: border-box;
    width: max-content;
    min-width: 150px;
    max-width: 400px;
    height: 100%;
    padding-left: 10px;
    overflow: auto;
  }

  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  h3 {
    margin: 0;
    font-size: inherit;
    font-weight: 600;
    color: var(--text-strong);
  }

  .author {
    display: flex;
    gap: 8px;
    align-items: center;
    white-space: nowrap;
  }

  .author input {
    flex: 1;
    min-width: 120px;
  }
</style>
