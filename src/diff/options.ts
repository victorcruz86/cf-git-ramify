import type { MenuAction } from "../lib/menu";
import type { IgnorePolicy } from "./blocks";
import type { EditorOptions } from "./editor";

export interface DiffOptions extends EditorOptions {
  unified: boolean;
  ignore: IgnorePolicy;
}

// The places of DiffPlaces that rama has: the diff window, the commit dialog's `Diff` panel, the
// Local Changes preview, the merge window, the diff docked in the log (also in the push dialog) and
// the stash and shelf preview (`SavedPatchesUi.SAVED_PATCHES_UI_PLACE`, not one of DiffPlaces).
// Soft wrap is remembered for each one, as TextDiffSettingsHolder's PlaceSettings does.
export type DiffPlace =
  | "Default"
  | "CommitDialog"
  | "ChangesView"
  | "Merge"
  | "VcsLogView"
  | "SavedPatchesUiPlace";

const KEY = "diff.options";
const WRAP_KEY = "diff.wrap.";

// The default values of DiffSettings and TextDiffSettings.
const DEFAULTS: Omit<DiffOptions, "wrap"> = {
  unified: false,
  ignore: "none",
  highlight: "words",
  collapse: true,
  sync: true,
  align: false,
};

// `USE_SOFT_WRAPS` is false in every place of IntelliJ; the user's `Default` has it on.
const WRAP_DEFAULTS: Record<DiffPlace, boolean> = {
  Default: true,
  CommitDialog: false,
  ChangesView: false,
  Merge: false,
  VcsLogView: false,
  SavedPatchesUiPlace: false,
};

export function loadWrap(place: DiffPlace): boolean {
  try {
    const stored = localStorage.getItem(WRAP_KEY + place);
    if (stored === "true" || stored === "false") return stored === "true";
  } catch {
    // Without storage, the default.
  }
  return WRAP_DEFAULTS[place];
}

export function storeWrap(place: DiffPlace, on: boolean) {
  try {
    localStorage.setItem(WRAP_KEY + place, String(on));
  } catch {
    // Without storage, all that is lost is what is remembered between sessions.
  }
}

// The shared options, with the soft wrap of `place`.
export function loadOptions(place: DiffPlace = "Default"): DiffOptions {
  const wrap = loadWrap(place);
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}"), wrap };
  } catch {
    return { ...DEFAULTS, wrap };
  }
}

// Soft wrap is not shared: it goes with `storeWrap`.
export function storeOptions(options: DiffOptions) {
  try {
    localStorage.setItem(KEY, JSON.stringify({ ...options, wrap: undefined }));
  } catch {
    // Without storage, all that is lost is what is remembered between sessions.
  }
}

// `Soft-Wrap` (ActionsBundle `action.EditorToggleUseSoftWraps.text`), in the gear and in the gutter.
export function softWrapItem(wrap: boolean, onwrap: (on: boolean) => void): MenuAction {
  return { label: "Soft-Wrap", checked: wrap, run: () => onwrap(!wrap) };
}

// A right click on the line-number gutter gets the gutter popup instead of the text's menu.
export function inGutter(event: MouseEvent): boolean {
  return event.target instanceof Element && event.target.closest(".cm-gutters") !== null;
}
