import type { NewWindow, OpenProjectIn } from "../lib/api";

// What the Open Project dialog answered: `here` is This Window, `new` New Window. `remember` is
// the `Don't ask again` checkbox.
export interface OpenAnswer {
  choice: "here" | "new" | "cancel";
  remember: boolean;
}

// What the open flow needs from the backend and the calling window.
export interface OpenProjectDeps {
  // The repository root of `target` (`project_resolve`); it fails outside a repository.
  resolve(target: string): Promise<string>;
  // `true` if another window has `root` open and was brought to the front (`project_focus_open`).
  focusOpen(root: string): Promise<boolean>;
  // The root of the repository open in this window, if any.
  current(): string | null;
  setting(): OpenProjectIn;
  // Today's `repo_open` path in this window.
  openHere(root: string): Promise<void>;
  // A new main window that opens `root` (`project_new_window`). If another window got `root` in
  // the meantime, that one comes to the front instead and nothing is built (`created: false`).
  newWindow(root: string): Promise<NewWindow>;
  // The Open Project dialog, rendered by this window, for the project named `name`.
  ask(name: string): Promise<OpenAnswer>;
  // `Don't ask again`: saves the choice as `Open project in`.
  remember(choice: Exclude<OpenProjectIn, "ask">): Promise<void>;
  // Brings this window to the front, for the repository it already has open.
  focusHere(): Promise<void>;
}

// The name IntelliJ shows for a project: its root's folder.
export function projectName(root: string): string {
  return root.split("/").filter(Boolean).pop() ?? root;
}

// `FileUtil.getLocationRelativeToUserHome`: the path from `~` when it is inside the home folder.
export function homeRelative(path: string, home: string | null): string {
  if (home && path.startsWith(`${home}/`)) return `~${path.slice(home.length)}`;
  return path;
}

// `ProjectUtil.openOrImport` and `confirmOpenNewProject`, for every way a repository is opened
// (Open, a typed path, Recent Projects, Clone, Create Git Repository, a folder handed to the app):
// 1. the target's repository root;
// 2. if another window has it open, that window comes to the front and nothing opens;
// 3. if this window has it open, this window comes to the front and nothing opens again
//    (`findAndFocusExistingProjectForPath` matches the calling frame too): its auxiliary windows
//    and its commit draft stay;
// 4. a window without a repository opens it here;
// 5. otherwise `Open project in` says where: here, a new window, or the dialog asks.
// A failure (not a repository, a refused leave) is thrown to the caller, which shows it, except
// `open_elsewhere`: another window started opening the root meanwhile and was brought to the
// front (`isOpenElsewhere`).
export async function openProject(target: string, deps: OpenProjectDeps): Promise<void> {
  const root = await deps.resolve(target);
  if (await deps.focusOpen(root)) return;
  const current = deps.current();
  if (current === root) {
    await deps.focusHere();
    return;
  }
  if (current === null) {
    await deps.openHere(root);
    return;
  }
  const setting = deps.setting();
  if (setting === "current") {
    await deps.openHere(root);
    return;
  }
  if (setting === "new") {
    await deps.newWindow(root);
    return;
  }
  const answer = await deps.ask(projectName(root));
  if (answer.choice === "cancel") return;
  if (answer.remember) await deps.remember(answer.choice === "here" ? "current" : "new");
  if (answer.choice === "here") await deps.openHere(root);
  else await deps.newWindow(root);
}

// `repo_open` found the root open, or being opened, in another window, which came to the front.
export const isOpenElsewhere = (failure: unknown) =>
  typeof failure === "object" &&
  failure !== null &&
  "kind" in failure &&
  failure.kind === "open_elsewhere";
