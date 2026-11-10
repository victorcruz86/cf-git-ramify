import { clipboardWrite, githubLinks, openUrl, type LinkReference, type LinkTarget } from "../lib/api";
import { errorText } from "../lib/errors";
import type { MenuItem } from "../lib/menu";
import { notify } from "../lib/notifications";

// `GitB:1560`, the title of every failure here.
export const CANNOT_OPEN = "Cannot open in browser";

// The places' labels (`GB:73-77`): the group's own text is `Open on GitHub`, in the `Open In` submenu
// it is `GitHub`, and the copy group is `Copy Link to GitHub Repository`.
export const OPEN_ON_GITHUB = "Open on GitHub";
export const OPEN_IN_GITHUB = "GitHub";
export const COPY_LINK = "Copy Link to GitHub Repository";

function defaultFail(failure: unknown) {
  notify({ kind: "error", title: CANNOT_OPEN, body: errorText(failure) });
}

// What `githubLinks` answered for a target: its references, or why they could not be found.
export type HostingLinks =
  | { references: LinkReference[]; failure?: undefined }
  | { references?: undefined; failure: unknown };

// One look-up, for a menu that shows both the open and the copy items.
export async function hostingLinks(target: LinkTarget): Promise<HostingLinks> {
  try {
    return { references: await githubLinks(target) };
  } catch (failure) {
    return { failure };
  }
}

// `HostedGitRepositoryReferenceActionGroup`: nothing when no GitHub repository answers (no remote
// of a known server, or a file with no commit), one item for one repository and a submenu with one
// item per repository, named by its address, for several. `open` opens the browser, `copy` puts the
// URL on the clipboard. A failure to find the references still shows the item, as 241 does, and
// reports itself when used. `fail` shows an error in the window that owns the menu.
export async function hostingItems(
  target: LinkTarget,
  kind: "open" | "copy",
  label: string,
  fail: (failure: unknown) => void = defaultFail,
): Promise<MenuItem[]> {
  return hostingItemsOf(await hostingLinks(target), kind, label, fail);
}

export function hostingItemsOf(
  links: HostingLinks,
  kind: "open" | "copy",
  label: string,
  fail: (failure: unknown) => void = defaultFail,
): MenuItem[] {
  const act = (url: string) => () =>
    void (kind === "open" ? openUrl(url) : clipboardWrite(url)).catch(fail);
  const { references, failure } = links;
  if (!references) return [{ label, run: () => fail(failure) }];
  if (references.length === 0) return [];
  if (references.length === 1) return [{ label, run: act(references[0].url) }];
  return [
    {
      label,
      submenu: references.map((reference) => ({
        label: reference.name,
        run: act(reference.url),
      })),
    },
  ];
}
