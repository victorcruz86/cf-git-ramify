import { mount } from "svelte";
import MergeWindow from "./MergeWindow.svelte";

export const MERGE_WINDOW = "merge";
export const MERGE_WINDOW_URL = "index.html?window=merge";

export function mountMergeWindow(target: HTMLElement) {
  mount(MergeWindow, { target });
}
