<script lang="ts" module>
  // New UI icons (platform/icons/src/expui, Apache 2.0). The gray of their strokes becomes
  // `currentColor` so that the control using them decides the color.
  const files = import.meta.glob<string>("./icons/*.svg", {
    query: "?raw",
    import: "default",
    eager: true,
  });
  const icons = new Map(
    Object.entries(files).map(([path, svg]) => [
      path.slice("./icons/".length, -".svg".length),
      svg.replace(/<!--.*?-->\s*/s, "").replaceAll("#CED0D6", "currentColor"),
    ]),
  );
</script>

<script lang="ts">
  let { name, size = 16 }: { name: string; size?: number } = $props();
</script>

<span class="icon" aria-hidden="true" style:width="{size}px" style:height="{size}px">
  {@html icons.get(name) ?? ""}
</span>

<style>
  .icon {
    display: inline-flex;
    flex: none;
    align-items: center;
    justify-content: center;
    color: var(--icon);
  }

  .icon :global(svg) {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
