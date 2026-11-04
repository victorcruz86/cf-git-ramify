import { get } from "svelte/store";
import { afterEach, expect, test, vi } from "vitest";
import type { CheckinRequest } from "./checkin";
import {
  closeCommitDialog,
  commitDialog,
  requestCommit,
  routeCommitRequests,
} from "./dialogStore";

const REQUEST: CheckinRequest = { kind: "project", paths: ["a.txt"], lists: ["default"] };

afterEach(() => {
  closeCommitDialog();
  routeCommitRequests(null);
});

test("in modal mode a request opens the dialog with the prefilled text and the rebase base", () => {
  const nonModal = vi.fn();
  routeCommitRequests({ mode: () => "modal", nonModal });

  requestCommit({ request: REQUEST, prefilled: "fixup! one", rebaseFrom: "abc~", head: "head" });

  expect(get(commitDialog)).toEqual({
    request: REQUEST,
    executor: "commit",
    prefilled: "fixup! one",
    rebaseFrom: "abc~",
    head: "head",
  });
  expect(nonModal).not.toHaveBeenCalled();
});

test("in non-modal mode a request goes to the Commit view and opens no dialog", () => {
  const nonModal = vi.fn();
  routeCommitRequests({ mode: () => "non-modal", nonModal });

  requestCommit({ request: REQUEST, prefilled: "squash! one" });

  expect(nonModal).toHaveBeenCalledWith({ request: REQUEST, prefilled: "squash! one" });
  expect(get(commitDialog)).toBeNull();
});

test("the mode is read at the time of each request", () => {
  let mode: "modal" | "non-modal" = "non-modal";
  const nonModal = vi.fn();
  routeCommitRequests({ mode: () => mode, nonModal });

  requestCommit({ request: REQUEST });
  mode = "modal";
  requestCommit({ request: REQUEST });

  expect(nonModal).toHaveBeenCalledTimes(1);
  expect(get(commitDialog)?.request).toEqual(REQUEST);
});

test("while a dialog is open another request does nothing", () => {
  routeCommitRequests({ mode: () => "modal", nonModal: vi.fn() });

  requestCommit({ request: REQUEST, prefilled: "first" });
  requestCommit({ request: REQUEST, prefilled: "second" });

  expect(get(commitDialog)?.prefilled).toBe("first");
});
