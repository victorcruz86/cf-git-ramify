import { store, stored } from "../lib/storage";

// `GitVcsSettings.SIGN_OFF_COMMIT`, per repository as IntelliJ keeps it per project. It seeds
// `Sign-off commit` in the Commit Changes dialog and in the Commit view, and both save it when a
// commit is attempted (`GitCommitOptionsUi.saveState`).
const SIGN_OFF_KEY = "GitVcsSettings.SIGN_OFF_COMMIT";

export const signOffKey = (repo: string) => `${SIGN_OFF_KEY}:${repo}`;

export function storedSignOff(repo: string): boolean {
  return stored(signOffKey(repo)) === "true";
}

export function storeSignOff(repo: string, value: boolean) {
  store(signOffKey(repo), String(value));
}
