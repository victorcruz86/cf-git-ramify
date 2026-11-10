import { get, writable } from "svelte/store";
import {
  githubAccountDetails,
  githubAccounts,
  githubAccountsWithoutTokens,
  githubRemove,
  githubSetDefault,
  repoSettingsGet,
  type AccountView,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";

// The accounts of Settings > GitHub. The rows come at once, and each one's details (the full name
// GitHub gave, or what went wrong asking for it) arrive on their own. `null` until the first load.
// A token is never here: the backend does not give it back.
interface AccountsState {
  accounts: AccountView[] | null;
  // Why the last load of the list failed.
  failure: unknown;
}

const state = writable<AccountsState>({ accounts: null, failure: null });
let loads = 0;

function replace(row: AccountView) {
  state.update((current) => ({
    ...current,
    accounts:
      current.accounts?.map((old) => (old.id === row.id ? row : old)) ?? null,
  }));
}

async function loadDetails(run: number, id: string) {
  try {
    const row = await githubAccountDetails(id);
    if (run === loads) replace(row);
  } catch (failure) {
    if (run !== loads) return;
    const row = get(state).accounts?.find((old) => old.id === id);
    if (row) replace({ ...row, error: errorText(failure), needsLogin: false });
  }
}

export const githubAccountsStore = {
  subscribe: state.subscribe,
  // A later load wins over an earlier one, including the details still coming for it.
  async load(): Promise<void> {
    const run = ++loads;
    try {
      const accounts = await githubAccounts();
      if (run !== loads) return;
      state.set({ accounts, failure: null });
      for (const { id } of accounts) void loadDetails(run, id);
    } catch (failure) {
      if (run === loads) state.update((current) => ({ ...current, failure }));
    }
  },
  // Forgets the list, so that the next one shows nothing of the last.
  reset(): void {
    loads++;
    state.set({ accounts: null, failure: null });
  },
  // `−`. There is no confirmation, as in 241.
  async remove(id: string): Promise<void> {
    await githubRemove(id);
    await githubAccountsStore.load();
  },
  async setDefault(id: string): Promise<void> {
    await githubSetDefault(id);
    await githubAccountsStore.load();
  },
};

// What a request for a login looks like: GHLoginDialog.Token and its callers (P§2.1).
export interface LoginRequest {
  // The server of the request, or `null` when it carries none.
  server: string | null;
  // The id of the account a row's `Log in` re-logs into.
  editing: string | null;
  // From the settings `+` menu: `Add GitHub Account`.
  add: boolean;
}

export interface LoginVariant {
  title: string;
  ok: string;
  server: string;
  serverEditable: boolean;
}

export const GITHUB_COM = "github.com";

export function loginVariant({
  server,
  editing,
  add,
}: LoginRequest): LoginVariant {
  if (add) {
    // `Log In with Token…` brings github.com; `Log In to GitHub Enterprise…` an empty field.
    return {
      title: "Add GitHub Account",
      ok: "Add Account",
      server: server ?? "",
      serverEditable: server === null,
    };
  }
  return {
    title: "Log In to GitHub",
    ok: "Log In",
    server: server ?? GITHUB_COM,
    serverEditable: server === null && editing === null,
  };
}

interface Pending extends LoginRequest {
  finish: (account: AccountView | null, maybeSaved?: boolean) => void;
}

// The dialog that `LoginHost` shows.
export const loginRequest = writable<Pending | null>(null);

// Who wants to know that an account was logged into, wherever it was asked (Settings or the Pull
// Requests window): the window forgets then that the account had no token.
const loginListeners = new Set<(account: AccountView) => void>();

export function onLogin(listener: (account: AccountView) => void): () => void {
  loginListeners.add(listener);
  return () => loginListeners.delete(listener);
}

export function announceLogin(account: AccountView): void {
  loginListeners.forEach((listener) => listener(account));
}

// Opens the login dialog and gives the account it logged into, or `null` if it was cancelled. Used by
// the accounts list and by the Pull Requests window. `server`: the request's server, if it has
// one; `editing`: the account being logged into again; `add`: it comes from the settings `+`.
export function loginFor(
  server: string | null,
  editing?: string,
  options: { add?: boolean } = {},
): Promise<AccountView | null> {
  // The dialog is modal: a second one cannot be asked for while one is open.
  if (get(loginRequest)) return Promise.resolve(null);
  return new Promise((resolve) => {
    loginRequest.set({
      server,
      editing: editing ?? null,
      add: options.add ?? false,
      finish: (account, maybeSaved = false) => {
        loginRequest.set(null);
        if (account) announceLogin(account);
        // A cancelled login may have saved the account already.
        if (account || maybeSaved) void githubAccountsStore.load();
        resolve(account);
      },
    });
  });
}

// `GithubProjectDefaultAccountHolder`: the default of the open repository is an account that is no
// longer there. The list of accounts makes no request and reads no token. `current` says whether
// the repository is still the one that was opened: an answer for another one is dropped.
export async function checkDefaultAccount(
  configure: () => void,
  current: () => boolean,
): Promise<void> {
  try {
    const stored = (await repoSettingsGet()).githubDefaultAccount;
    if (!stored) return;
    const accounts = await githubAccountsWithoutTokens();
    if (!current() || accounts.some((account) => account.id === stored)) return;
    notify({
      kind: "warning",
      title: "Missing default GitHub account",
      actions: [{ label: "Configure…", run: configure }],
    });
  } catch (failure) {
    console.warn(
      "Could not check the default GitHub account:",
      errorText(failure),
    );
  }
}
