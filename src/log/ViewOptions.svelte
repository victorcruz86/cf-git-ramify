<script lang="ts" module>
  import type { SortType } from "../lib/api";
  import type { MenuAction, MenuItem } from "../lib/Menu.svelte";
  import { collapseLabel, expandLabel } from "./labels";
  import { toggleColumn, type Highlights, type LogColumn, type LogOptions } from "./options";
  import { SIGNATURE_COLUMN } from "./signatures";

  const COLUMNS: [LogColumn, string][] = [
    ["author", "Author"],
    ["date", "Date"],
    ["hash", "Hash"],
    // `column.name.commit.signature` (`GitB:1084`): external-status columns come after the others.
    ["signature", SIGNATURE_COLUMN],
  ];
  const SORTS: [SortType, string][] = [
    ["off", "Off"],
    ["standard", "Standard"],
    ["linear", "Linear"],
  ];
  const HIGHLIGHTS: [keyof Highlights, string][] = [
    ["myCommits", "My Commits"],
    ["mergeCommits", "Merge Commits"],
    ["currentBranch", "Current Branch"],
  ];

  // `Git.Log.DeepCompare` (B:557).
  export const NOT_PICKED = "Not Cherry-Picked Commits";

  export interface ViewActions {
    onoptions: (change: Partial<LogOptions>) => void;
    onsort: (sort: SortType) => void;
    onlongedges: (show: boolean) => void;
    oncollapse: (collapsed: boolean) => void;
    // `DeepCompareAction`: checked while a comparison runs or highlights. Only the logs that are a
    // `MainVcsLogUi` (the Log's own tabs) have it.
    notPicked?: { checked: boolean; toggle: () => void };
  }

  // The order of `Vcs.Log.PresentationSettings`. `Root Names` has no effect with one repository.
  export function viewOptionItems(
    options: LogOptions,
    collapsible: boolean,
    empty: boolean,
    { onoptions, onsort, onlongedges, oncollapse, notPicked }: ViewActions,
  ): MenuItem[] {
    const toggle = (label: string, key: "compactRefs" | "tagNames" | "preferCommitDate" | "refsOnLeft") => ({
      label,
      checked: options[key],
      run: () => onoptions({ [key]: !options[key] }),
    });
    const collapse: MenuAction[] = [
      { label: collapseLabel(options.sort), disabled: empty, run: () => oncollapse(true) },
      { label: expandLabel(options.sort), disabled: empty, run: () => oncollapse(false) },
    ];
    return [
      { header: "Show" },
      toggle("Compact References View", "compactRefs"),
      toggle("Tag Names", "tagNames"),
      {
        label: "Long Edges",
        checked: options.longEdges,
        run: () => onlongedges(!options.longEdges),
      },
      toggle("Prefer Commit Timestamp", "preferCommitDate"),
      toggle("References on the Left", "refsOnLeft"),
      {
        label: "Columns",
        submenu: COLUMNS.map(([column, label]) => ({
          label,
          checked: options.columns.includes(column),
          run: () => onoptions({ columns: toggleColumn(options.columns, column) }),
        })),
      },
      { header: "Highlight" },
      ...HIGHLIGHTS.map(([key, label]) => ({
        label,
        checked: options.highlights[key],
        run: () => onoptions({ highlights: { ...options.highlights, [key]: !options.highlights[key] } }),
      })),
      // `X:328-331`: right after the `Highlight` section, before `Branches`.
      ...(notPicked
        ? [
            {
              label: NOT_PICKED,
              checked: notPicked.checked,
              run: notPicked.toggle,
            },
          ]
        : []),
      { header: "Branches" },
      {
        label: "IntelliSort",
        submenu: SORTS.map(([sort, label]) => ({
          label,
          checked: options.sort === sort,
          run: () => onsort(sort),
        })),
      },
      ...(collapsible ? collapse : []),
    ];
  }
</script>

<script lang="ts">
  import Menu from "../lib/Menu.svelte";

  interface Props extends ViewActions {
    x: number;
    y: number;
    options: LogOptions;
    collapsible: boolean;
    empty: boolean;
    onclose: () => void;
  }

  let { x, y, options, collapsible, empty, onclose, ...actions }: Props = $props();
</script>

<Menu
  {x}
  {y}
  label="View Options"
  items={viewOptionItems(options, collapsible, empty, actions)}
  {onclose}
/>
