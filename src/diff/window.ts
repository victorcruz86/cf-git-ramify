import { mount } from "svelte";
import DiffWindow from "./DiffWindow.svelte";

export function mountDiffWindow(target: HTMLElement) {
  mount(DiffWindow, { target });
}
