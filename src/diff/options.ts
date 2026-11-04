import type { IgnorePolicy } from "./blocks";
import type { EditorOptions } from "./editor";

export interface DiffOptions extends EditorOptions {
  unified: boolean;
  ignore: IgnorePolicy;
}

const KEY = "diff.options";

// The default values of DiffSettings and TextDiffSettings.
const DEFAULTS: DiffOptions = {
  unified: false,
  ignore: "none",
  highlight: "words",
  collapse: true,
  sync: true,
  align: false,
};

export function loadOptions(): DiffOptions {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
  } catch {
    return DEFAULTS;
  }
}

export function storeOptions(options: DiffOptions) {
  try {
    localStorage.setItem(KEY, JSON.stringify(options));
  } catch {
    // Without storage, all that is lost is what is remembered between sessions.
  }
}
