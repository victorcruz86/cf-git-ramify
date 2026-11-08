import { open, save, type DialogFilter } from "@tauri-apps/plugin-dialog";

// The native choosers (NSOpenPanel, NSSavePanel) of `tauri-plugin-dialog`. Each one gives the
// chosen path, or `null` if the user cancelled. Whoever reads or writes that path checks it in the
// backend.
export interface ChooserOptions {
  title?: string;
  // The folder it starts in, or the file it starts with selected.
  defaultPath?: string;
  filters?: DialogFilter[];
}

const single = (chosen: string | string[] | null): string | null =>
  Array.isArray(chosen) ? (chosen[0] ?? null) : chosen;

export async function chooseFile(options: ChooserOptions = {}): Promise<string | null> {
  return single(await open({ ...options, multiple: false, directory: false }));
}

export async function chooseFolder(
  options: Omit<ChooserOptions, "filters"> = {},
): Promise<string | null> {
  return single(await open({ ...options, multiple: false, directory: true }));
}

export async function chooseSaveFile(options: ChooserOptions = {}): Promise<string | null> {
  return await save(options);
}

// Several files at once (`Import Patches…`). NSOpenPanel here picks files only.
export async function chooseFiles(options: ChooserOptions = {}): Promise<string[]> {
  const chosen = await open({ ...options, multiple: true, directory: false });
  if (chosen === null) return [];
  return Array.isArray(chosen) ? chosen : [chosen];
}
