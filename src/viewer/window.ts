import { mount } from "svelte";
import { viewerOpen, type ViewerTarget } from "../lib/api";
import type { LineRange } from "./editor";

export interface ViewerSelection extends LineRange {
  path: string;
  revision: string | null;
}

// The viewer page says what its active tab has selected; outside that page there is none.
let selection: () => ViewerSelection | null = () => null;

// The page is loaded only in the viewer window, so the main one does not bring in its editor.
export async function mountViewerWindow(target: HTMLElement) {
  const { default: ViewerWindow } = await import("./ViewerWindow.svelte");
  mount(ViewerWindow, { target });
}

// Opens each file in its tab of the viewer window, which comes to the front.
export function openInViewer(targets: ViewerTarget | ViewerTarget[]): Promise<void> {
  return viewerOpen(Array.isArray(targets) ? targets : [targets]);
}

// The file and lines of the active tab, for the Git file menu, Annotate and selection history.
export function viewerSelection(): ViewerSelection | null {
  return selection();
}

export function provideViewerSelection(provider: () => ViewerSelection | null) {
  selection = provider;
  return () => {
    if (selection === provider) selection = () => null;
  };
}
