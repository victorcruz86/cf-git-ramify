<script lang="ts">
  import { stagingStage, stagingUnstage, type FileChange } from "../lib/api";
  import { errorText } from "../lib/errors";
  import ChangesTree from "./ChangesTree.svelte";
  import { hoverOperation, stagingTargets } from "./staging";
  import type { TreeNode } from "./tree";

  const NONE = new Set<string>();

  interface Props {
    roots: TreeNode[];
    changes: FileChange[];
    collapsed: Set<string>;
    selected: Set<string>;
    lead: string | null;
    onchanged: () => void;
    // There was something that existed only in the index: this is its snapshot.
    oncopy: (snapshot: string) => void;
    onerror: (text: string) => void;
    onopen: (node: TreeNode) => void;
    onmenu: (x: number, y: number) => void;
    onresolve?: () => void;
  }

  let {
    roots,
    changes,
    collapsed = $bindable(),
    selected = $bindable(),
    lead = $bindable(),
    onchanged,
    oncopy,
    onerror,
    onopen,
    onmenu,
    onresolve,
  }: Props = $props();

  let tree: ChangesTree;

  export function focus() {
    tree.focus();
  }

  export async function apply(targets: { stage: string[]; unstage: string[] }) {
    try {
      const copies = [
        targets.unstage.length > 0 ? await stagingUnstage(targets.unstage) : null,
        targets.stage.length > 0 ? await stagingStage(targets.stage) : null,
      ];
      copies.forEach((copy) => copy && oncopy(copy));
    } catch (failure) {
      onerror(errorText(failure));
    }
    onchanged();
  }
</script>

<ChangesTree
  bind:this={tree}
  {roots}
  included={NONE}
  bind:collapsed
  bind:selected
  bind:lead
  ontoggle={() => {}}
  {onopen}
  {onmenu}
  {onresolve}
  hoverIcon={hoverOperation}
  onhover={(node) => apply(stagingTargets(node.paths, changes))}
/>
