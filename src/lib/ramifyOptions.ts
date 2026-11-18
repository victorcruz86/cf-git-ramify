import { writable } from "svelte/store";
import type { Settings } from "./api";

// The options of the `Ramify` section of Settings, which IntelliJ does not have
// (`docs/parity-matrix.md`). App keeps them in step with the saved settings; until those are read,
// they are on, as they start.
export type RamifyOptions = Pick<
  Settings,
  "commitMessageTicket" | "notifyExternalChanges" | "copyBranchName" | "viewUpdatedBranchCommits"
>;

export const RAMIFY_DEFAULTS: RamifyOptions = {
  commitMessageTicket: true,
  notifyExternalChanges: true,
  copyBranchName: true,
  viewUpdatedBranchCommits: true,
};

export const ramifyOptions = writable<RamifyOptions>(RAMIFY_DEFAULTS);

export const optionsOf = (settings: Settings): RamifyOptions => ({
  commitMessageTicket: settings.commitMessageTicket,
  notifyExternalChanges: settings.notifyExternalChanges,
  copyBranchName: settings.copyBranchName,
  viewUpdatedBranchCommits: settings.viewUpdatedBranchCommits,
});
