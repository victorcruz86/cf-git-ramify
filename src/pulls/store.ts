import { get, writable } from "svelte/store";
import {
  githubForgetConnection,
  githubKnown,
  type AccountView,
  type Connection,
  type Known,
  type MappingView,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { githubAccountsStore, onLogin } from "../hosting/accounts";
import {
  activateWindow,
  registeredToolWindows,
  registerToolWindow,
  unregisterToolWindow,
} from "../shell/toolWindows";

// The list tab's id in the strip. A pull request's tab is its number, which starts at 1.
export const LIST_TAB = 0;

// What the Pull Requests window shows: the repositories and accounts it can choose from (`null`
// until the first check), the open connection, the pull requests open in tabs in the order they
// were opened, and the tab in front. `shown` turns on the first time the window comes to the
// front in this repository: until then nothing connects, as 241's `selectorVm` is `by lazy`.
// `tokenless` are the accounts a connect found without a token (`MissingCredentials`): the tokens
// are read only when connecting, so the selector learns it then, and keeps it when it is shown
// again; a login of the account forgets it.
export interface PullsState {
  known: Known | null;
  shown: boolean;
  connection: Connection | null;
  tabs: number[];
  selected: number;
  tokenless: ReadonlySet<string>;
}

const EMPTY: PullsState = {
  known: null,
  shown: false,
  connection: null,
  tabs: [],
  selected: LIST_TAB,
  tokenless: new Set(),
};
const state = writable<PullsState>(EMPTY);
export const pullsStore = { subscribe: state.subscribe };

// `URIUtil.equalWithoutSchema`: the server of an account and of a repository, without the scheme
// that only one of them may have been typed with.
const withoutScheme = (server: string) => server.replace(/^https?:\/\//i, "");
export const sameServer = (left: string, right: string) =>
  withoutScheme(left) === withoutScheme(right);

export const isGithubCom = (server: string) => sameServer(server, "github.com");

export const accountsOn = (known: Known, mapping: MappingView): AccountView[] =>
  known.accounts.filter((account) => sameServer(account.server, mapping.server));

// `singleRepoAndAccountState`: one repository and exactly one account on its server. The window then
// connects by itself and offers no `Change Repository or Account…`.
export function singlePair(known: Known): { mapping: MappingView; account: AccountView } | null {
  if (known.mappings.length !== 1) return null;
  const mapping = known.mappings[0];
  const here = accountsOn(known, mapping);
  return here.length === 1 ? { mapping, account: here[0] } : null;
}

// Whether the connection is still one the window could open: its remote and its account are known.
function stillOpen(connection: Connection, known: Known): boolean {
  return (
    known.mappings.some((mapping) => mapping.remote === connection.remote && mapping.url === connection.url) &&
    known.accounts.some((account) => account.id === connection.account)
  );
}

let checks = 0;
// Bumped whenever the window is forgotten (another repository opened, or no GitHub remote left):
// whatever was asked before, a connect or a login, finishes for a window that is gone.
let generation = 0;

export function pullsGeneration(): number {
  return generation;
}

// `isAvailable`: the window exists while a remote points to a GitHub repository. Asked when a
// repository opens and on every `repo-changed`; `current` says whether the answer is still for the
// open repository. A connection whose remote or account is gone is closed, its tabs with it.
export async function checkPullRequestsWindow(current: () => boolean = () => true): Promise<void> {
  const run = ++checks;
  const asked = generation;
  let known: Known;
  try {
    known = await githubKnown();
  } catch (failure) {
    console.warn("Could not read the GitHub repositories of the remotes:", errorText(failure));
    return;
  }
  if (run !== checks || asked !== generation || !current()) return;
  if (known.mappings.length === 0) {
    unregisterToolWindow("pulls");
    reset();
    return;
  }
  state.update((now) => {
    const sameKnown = JSON.stringify(now.known) === JSON.stringify(known);
    const connection = now.connection && stillOpen(now.connection, known) ? now.connection : null;
    if (sameKnown && connection === now.connection) return now;
    return connection
      ? { ...now, known }
      : { ...now, known, connection: null, tabs: [], selected: LIST_TAB };
  });
  registerToolWindow("pulls");
}

function reset() {
  generation++;
  if (get(state) !== EMPTY) state.set(EMPTY);
}

// The window came to the front: from now on it connects (the saved choice, the automatic pair).
export function showPullRequestsWindow(): void {
  if (!get(state).shown) state.update((now) => ({ ...now, shown: true }));
}

export function currentConnection(): Connection | null {
  return get(state).connection;
}

// Whether a URL that GitHub sent may open in the browser (the Task 2 ruling): `https://`, or
// `http://` when the account's server is an `http://` one, on the connection's server host and
// port, with no user or password in it.
export function openableUrl(connection: Connection, url: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  const server = connection.coordinates.server;
  const schemes = server.useHttp ? ["https:", "http:"] : ["https:"];
  const defaultPort = parsed.protocol === "http:" ? "80" : "443";
  const port = parsed.port || defaultPort;
  const expected = server.port === null ? defaultPort : String(server.port);
  return (
    schemes.includes(parsed.protocol) &&
    parsed.hostname.toLowerCase() === server.host.toLowerCase() &&
    port === expected &&
    parsed.username === "" &&
    parsed.password === ""
  );
}

// Why `openableUrl` refused a URL, with the schemes the connection's server allows.
export function refusedUrlText(connection: Connection): string {
  const server = connection.coordinates.server;
  const schemes = server.useHttp ? "an https:// or http://" : "an https://";
  const host = server.port === null ? server.host : `${server.host}:${server.port}`;
  return `Only ${schemes} address on ${host} can be opened`;
}

// A connect that succeeded: the list tab, and nothing else open yet. `asked` is the generation the
// connect started in; one for a window that has been forgotten since is dropped.
export function connected(connection: Connection, asked: number = generation): void {
  if (asked !== generation) return;
  state.update((now) => ({ ...now, connection, tabs: [], selected: LIST_TAB }));
}

// `GHPRToolWindowViewModel.resetRemoteAndAccount`: the saved choice is forgotten and the connection
// closed, which brings the selector back.
export async function changeRepositoryOrAccount(): Promise<void> {
  state.update((now) => ({
    ...now,
    known: now.known && { ...now.known, saved: null },
    connection: null,
    tabs: [],
    selected: LIST_TAB,
  }));
  await githubForgetConnection();
}

// A pull request's tab: one per number, opened again if it is there, and brought to the front.
export function openPullRequestTab(number: number): void {
  if (!get(state).connection) return;
  state.update((now) => ({
    ...now,
    tabs: now.tabs.includes(number) ? now.tabs : [...now.tabs, number],
    selected: number,
  }));
  activateWindow("pulls");
}

// Closing the tab in front shows the one before it, as the content manager does.
export function closePullRequestTab(number: number): void {
  state.update((now) => {
    const index = now.tabs.indexOf(number);
    if (index < 0) return now;
    const tabs = now.tabs.filter((other) => other !== number);
    const selected = now.selected === number ? (now.tabs[index - 1] ?? LIST_TAB) : now.selected;
    return { ...now, tabs, selected };
  });
}

// `View Pull Requests` (`GithubViewPullRequestsAction`): `activateAndAwaitProject { viewList() }`,
// the window to the front on its list tab. Without a connection the selector is what it shows.
export function viewPullRequestList(): void {
  selectPullsTab(LIST_TAB);
  activateWindow("pulls");
}

export function markTokenless(account: string): void {
  if (get(state).tokenless.has(account)) return;
  state.update((now) => ({ ...now, tokenless: new Set([...now.tokenless, account]) }));
}

export function forgetTokenless(account: string): void {
  if (!get(state).tokenless.has(account)) return;
  state.update((now) => ({
    ...now,
    tokenless: new Set([...now.tokenless].filter((id) => id !== account)),
  }));
}

// A login anywhere, Settings included, gave the account a token.
onLogin((account) => forgetTokenless(account.id));

export function selectPullsTab(id: number): void {
  state.update((now) => (now.selected === id ? now : { ...now, selected: id }));
}

// Opening another repository forgets the window, and its connection with it. Only the window
// going away counts: another window registering must not drop a check on its way.
let registered = false;
registeredToolWindows.subscribe((ids) => {
  const now = ids.includes("pulls");
  if (registered && !now) reset();
  registered = now;
});

// A login or a removal in Settings changes which servers are known (decision 4) and which accounts
// there are: the window is checked again, once a repository has been checked.
let accountsSeen = "";
githubAccountsStore.subscribe(({ accounts }) => {
  const seen = JSON.stringify(accounts?.map((account) => account.id) ?? null);
  if (seen === accountsSeen) return;
  const first = accountsSeen === "";
  accountsSeen = seen;
  if (!first && checks > 0 && accounts !== null) void checkPullRequestsWindow();
});
