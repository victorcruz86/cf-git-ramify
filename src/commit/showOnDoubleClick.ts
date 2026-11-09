import type { MenuItem } from "../lib/menu";
import { store, stored } from "../lib/storage";

// `VcsApplicationSettings.SHOW_DIFF_ON_DOUBLE_CLICK` (`VcsApplicationSettings.java:39`, default
// false): a double click or Enter on a file of Local Changes opens its source; on, the diff. Kept
// under IntelliJ's name, as the other view options are.
const KEY = "VcsApplicationSettings.SHOW_DIFF_ON_DOUBLE_CLICK";

export const SHOW_ON_DOUBLE_CLICK = "Show on Double-Click";
// `ActionsBundle.properties:1602-1606`: the texts of the tool window popup (`ToolwindowPopup`).
export const DIFF = "Diff";
export const SOURCE = "Source";

export function showDiffOnDoubleClick(): boolean {
  return stored(KEY) === "true";
}

function setShowDiffOnDoubleClick(value: boolean) {
  store(KEY, String(value));
}

// `LocalChangesView.GearActions` (`VcsActions.xml:624-631`): the `Show on Double-Click` group,
// `ShowOnDoubleClickToggleAction.EditorPreview` and `.Source` as options of one choice: in a
// popup, choosing one turns it on (`ShowOnDoubleClickToggleAction.kt:22-24`).
export function showOnDoubleClickItems(): MenuItem[] {
  const diff = showDiffOnDoubleClick();
  return [
    {
      label: SHOW_ON_DOUBLE_CLICK,
      submenu: [
        { label: DIFF, checked: diff, run: () => setShowDiffOnDoubleClick(true) },
        { label: SOURCE, checked: !diff, run: () => setShowDiffOnDoubleClick(false) },
      ],
    },
  ];
}
