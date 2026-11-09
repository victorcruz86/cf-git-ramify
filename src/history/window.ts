import { mount } from "svelte";
import { selectionHistoryOpen } from "../lib/api";

// The page is loaded only in its own window, so the others do not bring in its diff.
export async function mountSelectionHistoryWindow(target: HTMLElement) {
  const { default: SelectionHistoryWindow } = await import("./SelectionHistoryWindow.svelte");
  mount(SelectionHistoryWindow, { target });
}

// `SelectedBlockHistoryAction.showHistoryForSelection`: the local file's lines from the first to
// the last selected (given from 1, as the viewer counts them), with the text the viewer shows.
export function openSelectionHistory(
  path: string,
  startLine: number,
  endLine: number,
  localText: string,
): Promise<void> {
  return selectionHistoryOpen({
    path,
    startLine: Math.min(startLine, endLine) - 1,
    endLine: Math.max(startLine, endLine) - 1,
    localText,
    stream: 0,
  });
}
