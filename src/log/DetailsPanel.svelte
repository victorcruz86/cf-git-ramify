<script lang="ts" module>
  export interface MessagePart {
    text: string;
    hash?: string;
  }

  // What the backend recognized as commits of the log becomes a link; the rest is text.
  export function messageParts(text: string, links: Record<string, string>): MessagePart[] {
    const names = Object.keys(links).sort((a, b) => b.length - a.length);
    if (names.length === 0) return [{ text }];
    const parts: MessagePart[] = [];
    let rest = text;
    while (rest) {
      const found = names
        .map((name) => ({ name, index: rest.indexOf(name) }))
        .filter(({ index }) => index >= 0)
        .sort((a, b) => a.index - b.index)[0];
      if (!found) break;
      if (found.index > 0) parts.push({ text: rest.slice(0, found.index) });
      parts.push({ text: found.name, hash: links[found.name] });
      rest = rest.slice(found.index + found.name.length);
    }
    if (rest) parts.push({ text: rest });
    return parts;
  }
</script>

<script lang="ts">
  import { onDestroy } from "svelte";
  import {
    branchesContaining,
    commitDetails,
    type CommitDetails,
    type LogRef,
    type Signature,
  } from "../lib/api";
  import { errorText } from "../lib/errors";
  import Icon from "../lib/Icon.svelte";
  import { refColor } from "./labels";
  import {
    FINGERPRINT,
    SIGNED_BY,
    SignatureLoader,
    signatureDescription,
    signatureIcon,
    signatureText,
  } from "./signatures";

  // `CommitDetailsListPanel`.
  const MAX_COMMITS = 50;
  const SHORT_HASH = 8;
  const NO_COMMITS = "No commits selected";
  const LOADING = "In branches: loading…";
  const NOT_IN_BRANCH = "Not in any branch";

  const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short" });
  const timeFormat = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });

  interface Props {
    hashes: string[];
    // Changes with each view: a commit's references can be different after a reload.
    generation: number;
    onnavigate: (hash: string) => void;
    // `In branches:`. The push dialog does not show it, and there the log may not be loaded.
    showBranches?: boolean;
  }

  let { hashes, generation, onnavigate, showBranches = true }: Props = $props();

  // A prop is re-read on any change in the parent, and this launches git.
  const version = $derived(generation);

  let commits = $state.raw<CommitDetails[]>([]);
  let branches = $state.raw<Record<string, string[]>>({});
  let signed = $state.raw<Record<string, Signature>>({});
  // `VcsLogCommitSelectionListenerForDetails` asks for the signatures of the selection, whether the
  // column is shown or not. Only the log's own panels do: the dialogs, which have no `In branches:`
  // either, do not.
  const signatures = new SignatureLoader();
  let expanded = $state.raw<Set<string>>(new Set());
  let failure = $state<string | null>(null);
  let request = 0;

  const moment = (seconds: number) => {
    const date = seconds * 1000;
    return `on ${dateFormat.format(date)} at ${timeFormat.format(date)}`;
  };

  const person = (name: string, email: string) => (email ? `${name} <${email}>` : name);

  const authorLine = (commit: CommitDetails) =>
    `${commit.hash.slice(0, SHORT_HASH)} ${person(commit.author, commit.authorEmail)} ${moment(commit.authorTime)}`;

  // `CommitPresentationUtil.getAuthorAndCommitterText`.
  function committerLine(commit: CommitDetails): string | null {
    const other = commit.commitTime !== commit.authorTime;
    if (commit.samePerson) return other ? `committed ${moment(commit.commitTime)}` : null;
    const by = `committed by ${person(commit.committer, commit.committerEmail)}`;
    return other ? `${by} ${moment(commit.commitTime)}` : by;
  }

  const branchRefs = (commit: CommitDetails) => commit.refs.filter((ref) => ref.kind !== "tag");
  const tagRefs = (commit: CommitDetails) => commit.refs.filter((ref) => ref.kind === "tag");

  const branchesHead = (count: number) =>
    count === 1 ? "In 1 branch:" : `In ${count} branches:`;

  function toggleBranches(hash: string) {
    const next = new Set(expanded);
    if (!next.delete(hash)) next.add(hash);
    expanded = next;
  }

  async function loadBranches(token: number, shown: CommitDetails[]) {
    for (const { hash } of shown) {
      const found = await branchesContaining(hash).catch(() => []);
      if (token !== request) return;
      branches = { ...branches, [hash]: found };
    }
  }

  async function loadSignatures(token: number, shown: CommitDetails[]) {
    const found = await signatures.load(shown.map(({ hash }) => hash));
    if (token !== request || !found) return;
    signed = Object.fromEntries(found.map((signature) => [signature.hash, signature]));
  }

  async function load(selected: string[]) {
    const token = ++request;
    failure = null;
    expanded = new Set();
    if (selected.length === 0) {
      commits = [];
      branches = {};
      signed = {};
      signatures.cancel();
      return;
    }
    try {
      const loaded = await commitDetails(selected.slice(0, MAX_COMMITS));
      if (token !== request) return;
      const same = loaded.every((commit) => commits.some(({ hash }) => hash === commit.hash));
      commits = loaded;
      if (!same) {
        branches = {};
        signed = {};
      }
      if (showBranches) {
        loadSignatures(token, loaded);
        loadBranches(token, loaded);
      }
    } catch (error) {
      if (token === request) failure = errorText(error);
    }
  }

  $effect(() => {
    version;
    load(hashes);
  });

  onDestroy(() => signatures.cancel());
</script>

{#snippet message(text: string, links: Record<string, string>)}
  {#each messageParts(text, links) as part}
    {#if part.hash}
      {@const hash = part.hash}
      <button type="button" class="link" onclick={() => onnavigate(hash)}>{part.text}</button>
    {:else}
      {part.text}
    {/if}
  {/each}
{/snippet}

<!-- `HashAndAuthorPanel`: after the author and committer, in the context-help colour, the icon (whose
     tooltip is the description) and the text. An unsigned commit has none. -->
{#snippet signatureLine(signature: Signature)}
  {@const icon = signatureIcon(signature)}
  {@const description = signatureDescription(signature)}
  <p class="meta signature">
    <span class="signature-icon">
      {#if icon}<Icon name={icon} />{/if}
      <span class="tooltip" role="tooltip">
        <span class="line">{description.text}</span>
        {#if description.verified}
          <span class="context">
            <span class="line gap">{FINGERPRINT}</span>
            <span class="line">{description.verified.fingerprint}</span>
            <span class="line gap">{SIGNED_BY}</span>
            <span class="line">{description.verified.signer}</span>
          </span>
        {/if}
      </span>
    </span>
    <span>{signatureText(signature)}</span>
  </p>
{/snippet}

{#snippet references(refs: LogRef[])}
  {#if refs.length > 0}
    <p class="refs">
      {#each refs as ref, index (ref.name + ref.kind)}
        {#if index > 0},{" "}{/if}<span class="ref" style:--ref={refColor(ref.kind)}>{ref.name}</span>
      {/each}
    </p>
  {/if}
{/snippet}

<section class="details" aria-label="Commit details">
  {#if failure}
    <p class="status">{failure}</p>
  {:else if hashes.length === 0}
    <p class="status">{NO_COMMITS}</p>
  {:else}
    {#each commits as commit (commit.hash)}
      {@const found = branches[commit.hash]}
      {@const committed = committerLine(commit)}
      {@const open = expanded.has(commit.hash)}
      <article>
        <p class="message">
          {#if commit.subject.includes("\n")}
            {@render message(commit.subject, commit.links)}
          {:else}
            <strong>{@render message(commit.subject, commit.links)}</strong>
          {/if}
          {#if commit.body}
            <span class="body">{@render message(commit.body, commit.links)}</span>
          {/if}
        </p>
        <p class="meta">{authorLine(commit)}</p>
        {#if committed}
          <p class="meta committed">{committed}</p>
        {/if}
        {#if signed[commit.hash]}
          {@render signatureLine(signed[commit.hash])}
        {/if}
        {@render references(branchRefs(commit))}
        {@render references(tagRefs(commit))}
        {#if showBranches}
          {#if !found}
            <p class="meta">{LOADING}</p>
          {:else if found.length === 0}
            <p class="meta">{NOT_IN_BRANCH}</p>
          {:else if open}
            <p class="meta">
              <span>{branchesHead(found.length)}</span>
              <button type="button" class="link" onclick={() => toggleBranches(commit.hash)}>Hide</button>
            </p>
            <ul class="meta">
              {#each found as name (name)}
                <li>{name}</li>
              {/each}
            </ul>
          {:else}
            <p class="meta branches">
              <span class="list">{branchesHead(found.length)} {found.join(", ")}</span>
              <button type="button" class="link" onclick={() => toggleBranches(commit.hash)}>
                Show all
              </button>
            </p>
          {/if}
        {/if}
      </article>
    {/each}
    {#if hashes.length > commits.length && commits.length === MAX_COMMITS}
      <p class="meta overflow">(showing {MAX_COMMITS} of {hashes.length} selected commits)</p>
    {/if}
  {/if}
</section>

<style>
  .details {
    height: 100%;
    overflow: auto;
    color: var(--text);
    background: var(--panel);
  }

  article {
    padding: 8px 10px;
    border-bottom: 1px solid var(--divider);
  }

  p,
  ul {
    margin: 0 0 4px;
    padding: 0;
  }

  .status {
    padding: 16px;
    color: var(--text-secondary);
    text-align: center;
  }

  .message {
    margin-bottom: 8px;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .body {
    display: block;
    margin-top: 8px;
  }

  .meta {
    color: var(--text-secondary);
    overflow-wrap: anywhere;
  }

  .committed {
    color: var(--muted);
  }

  .signature {
    display: flex;
    gap: 4px;
    align-items: center;
    color: var(--muted);
  }

  .signature-icon {
    position: relative;
    display: inline-flex;
  }

  .tooltip {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 20;
    display: none;
    padding: 4px 8px;
    color: var(--text);
    white-space: nowrap;
    background: var(--popup-background);
    border: 1px solid var(--popup-border);
    border-radius: 6px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 40%);
  }

  .signature-icon:hover .tooltip {
    display: block;
  }

  .line {
    display: block;
  }

  .gap {
    margin-top: 1em;
  }

  .context {
    display: block;
    color: var(--muted);
  }

  ul {
    list-style: none;
  }

  .refs {
    color: var(--text-secondary);
  }

  .ref::before {
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: 4px;
    content: "";
    background: var(--ref);
    border-radius: 2px;
  }

  .branches {
    display: flex;
    gap: 4px;
    white-space: nowrap;
  }

  .list {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .link {
    padding: 0;
    font: inherit;
    color: var(--link);
    background: none;
    border: 0;
  }

  .overflow {
    padding: 8px 10px;
  }
</style>
