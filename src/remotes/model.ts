import {
  cloneRepository,
  initRepository,
  openExcludeFile,
  openInNewWindow,
  remoteAdd,
  remoteEdit,
  remoteList,
  remoteRemove,
  remoteTestUrl,
  type Cloned,
  type InitOutcome,
  type Remote,
} from "../lib/api";
import { errorText } from "../lib/errors";
import { notify } from "../lib/notifications";
import { taskResult } from "../lib/tasks";

const ORIGIN = "origin";

// What the backend does, already with the task's result.
export interface RemotesApi {
  list(): Promise<Remote[]>;
  add(name: string, url: string): Promise<unknown>;
  edit(name: string, newName: string, url: string): Promise<unknown>;
  remove(name: string): Promise<unknown>;
  // `remote`: the name it will be saved under, to mark the remote as authenticated.
  testUrl(url: string, remote: string | null): Promise<unknown>;
  clone(url: string, directory: string): Promise<Cloned>;
  init(path: string, confirmed: boolean): Promise<InitOutcome>;
  openExclude(): Promise<boolean>;
  openInNewWindow(path: string): Promise<void>;
}

const run = async <T>(id: Promise<number>) => taskResult<T>(await id);

export const remotesApi: RemotesApi = {
  list: remoteList,
  add: async (name, url) => run(remoteAdd(name, url)),
  edit: async (name, newName, url) => run(remoteEdit(name, newName, url)),
  remove: async (name) => run(remoteRemove(name)),
  testUrl: async (url, remote) => run(remoteTestUrl(url, remote)),
  clone: async (url, directory) => run<Cloned>(cloneRepository(url, directory)),
  init: async (path, confirmed) => run<InitOutcome>(initRepository(path, confirmed)),
  openExclude: openExcludeFile,
  openInNewWindow,
};

// `GitBundle`: remotes.define.*.validation.message.
const NAME_EMPTY = "Remote name cannot be empty";
const NAME_ILLEGAL = "Remote name contains illegal characters";
const URL_EMPTY = "Remote URL cannot be empty";
const nameUsed = (name: string) => `Remote name '${name}' is already used`;

// `GitRefNameValidator`: starts with `.`, `-` or `/`; contains `~:^?*"[\`, `@{`, `//`, `..`, `./` or
// `/.`; ends in `.`, `/` or `.lock`; or has spaces or control characters. Rust checks it again with
// `git check-ref-format`.
// eslint-disable-next-line no-control-regex
const ILLEGAL = /^[.\-/]|[~:^?*"[\\]|@\{|\/\/|\.\.|\.\/|\/\.|[./]$|\.lock$| |[\x00-\x1f\x7f]/;

export function remoteNameValid(name: string): boolean {
  return name.trim() !== "" && !ILLEGAL.test(name);
}

export interface RemoteProblems {
  name?: string;
  url?: string;
}

// `initial`: the name of the remote being edited, which does not count as a duplicate.
export function remoteProblem(
  name: string,
  url: string,
  taken: string[],
  initial?: string,
): RemoteProblems {
  const problems: RemoteProblems = {};
  const trimmed = name.trim();
  if (trimmed === "") problems.name = NAME_EMPTY;
  else if (!remoteNameValid(trimmed)) problems.name = NAME_ILLEGAL;
  else if (trimmed !== initial && taken.includes(trimmed)) problems.name = nameUsed(trimmed);
  if (url.trim() === "") problems.url = URL_EMPTY;
  return problems;
}

// The name that IntelliJ's `addRemote` proposes.
export const proposedName = (remotes: Remote[]) =>
  remotes.some((remote) => remote.name === ORIGIN) ? "" : ORIGIN;

export const REMOVE_TITLE = "Remove Remote";
export const removeMessage = ({ name, url }: Remote) => `Remove remote ${name} '${url}'?`;

// The password of `https://user:password@host` is not shown on screen.
const maskUrl = (url: string) => url.replace(/(:\/\/[^/@\s:]*):[^/@\s]*@/, "$1:***@");

async function attempt(
  title: string,
  message: string,
  work: () => Promise<unknown>,
): Promise<boolean> {
  try {
    await work();
    return true;
  } catch (failure) {
    notify({ kind: "error", title, body: `${message}\n${errorText(failure)}` });
    return false;
  }
}

// `true` if it was done. The error messages are those of `GitConfigureRemotesDialog`.
export const addRemote = (api: RemotesApi, name: string, url: string) =>
  attempt("Add Remote", `Cannot add remote ${name} '${maskUrl(url)}'`, () => api.add(name, url));

export async function editRemote(
  api: RemotesApi,
  remote: Remote,
  name: string,
  url: string,
): Promise<boolean> {
  if (name === remote.name && url === remote.url) return true;
  const message = `Cannot change remote ${remote.name} to ${name} '${maskUrl(url)}'`;
  return attempt("Change Remote", message, () => api.edit(remote.name, name, url));
}

export const removeRemote = (api: RemotesApi, remote: Remote) =>
  attempt(REMOVE_TITLE, `Cannot remove remote ${remote.name} '${maskUrl(remote.url)}'`, () =>
    api.remove(remote.name),
  );

// One repository per window. With an empty window it opens in it; with another one open, another
// system window.
export interface OpenDeps {
  // The root of the open repository, if any.
  current(): string | null;
  open(path: string): Promise<void>;
}

async function openRepository(deps: OpenDeps, api: RemotesApi, path: string) {
  try {
    if (deps.current() === null) await deps.open(path);
    else await api.openInNewWindow(path);
  } catch (failure) {
    notify({ kind: "error", title: "Cannot open repository", body: errorText(failure) });
  }
}

const isCancelled = (failure: unknown) =>
  typeof failure === "object" && failure !== null && "kind" in failure && failure.kind === "cancelled";

export function cloneFlow(deps: OpenDeps, api: RemotesApi = remotesApi) {
  return async (url: string, directory: string) => {
    let cloned: Cloned;
    try {
      cloned = await api.clone(url, directory);
    } catch (failure) {
      if (isCancelled(failure)) return;
      notify({ kind: "error", title: "Clone failed", body: errorText(failure) });
      return;
    }
    // A submodule that fails leaves the whole clone, and it is opened as IntelliJ does.
    if (cloned.warning) {
      notify({ kind: "warning", title: "Cloned, but a submodule failed", body: cloned.warning });
    }
    await openRepository(deps, api, cloned.path);
  };
}

export interface Confirmation {
  title: string;
  message: string;
}

export const underGit = (path: string): Confirmation => ({
  title: "Git Init",
  message: `The selected directory ${path} is already under Git.\nAre you sure that you want to create a new VCS root?`,
});

export function initFlow(
  deps: OpenDeps,
  ui: { confirm(request: Confirmation): Promise<boolean> },
  api: RemotesApi = remotesApi,
) {
  return async (path: string) => {
    let outcome: InitOutcome;
    try {
      outcome = await api.init(path, false);
      if (outcome.kind === "underGit") {
        if (!(await ui.confirm(underGit(path)))) return;
        outcome = await api.init(path, true);
      }
    } catch (failure) {
      if (isCancelled(failure)) return;
      notify({ kind: "error", title: "Git init failed", body: errorText(failure) });
      return;
    }
    if (outcome.kind !== "created" || outcome.path === deps.current()) return;
    await openRepository(deps, api, outcome.path);
  };
}

export async function openExclude(api: RemotesApi = remotesApi) {
  try {
    await api.openExclude();
  } catch (failure) {
    notify({ kind: "error", title: "Open .git/info/exclude", body: errorText(failure) });
  }
}
