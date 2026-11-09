import { mount, unmount } from "svelte";
import { compareCurrentRevision, type CurrentRevision } from "../lib/api";
import { runLocal } from "../lib/tasks";
import { prettyDateTime } from "../log/draw";
import CurrentRevisionPopup from "./CurrentRevisionPopup.svelte";

// `V:949`.
export const LOADING_CURRENT_REVISION = "Loading Current Revision…";

const nameOf = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// `current.version.text` (`V:611`) without its HTML: `{4}<br/><br/>Current version is {3}.<br/>
// Modified by {0}<br/>{1}<br/>{2}`, the message keeping its lines (`IssueLinkHtmlRenderer`).
export function currentRevisionLines(name: string, revision: CurrentRevision, now: number): string[] {
  return [
    name,
    "",
    `Current version is ${revision.hash}.`,
    `Modified by ${revision.author}`,
    prettyDateTime(revision.commitTime * 1000, now),
    ...revision.message.split("\n"),
  ];
}

// The popup shown now: a second Show Current Revision replaces it.
let closeOpen: (() => void) | null = null;

function showPopup(lines: string[]) {
  closeOpen?.();
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    if (closeOpen === close) closeOpen = null;
    // After the click or key that closes it has been handled.
    queueMicrotask(() => void unmount(instance));
  };
  const instance = mount(CurrentRevisionPopup, {
    target: document.body,
    props: { lines, onclose: close },
  });
  closeOpen = close;
}

// `ShowBaseRevisionAction`: the last commit of any branch that touched the file, read in the
// background; nothing is shown when there is none. A failure is the caller's to show.
export async function showCurrentRevision(path: string, now = Date.now()): Promise<void> {
  const revision = await runLocal(LOADING_CURRENT_REVISION, "", () => compareCurrentRevision(path));
  if (revision) showPopup(currentRevisionLines(nameOf(path), revision, now));
}
