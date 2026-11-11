import { beforeEach, expect, test, vi } from "vitest";

const listen = vi.fn(async () => () => {});
const emitTo = vi.fn(async () => {});

vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "diff-2" }),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (...args: unknown[]) => (listen as (...a: unknown[]) => unknown)(...args),
  emitTo: (...args: unknown[]) => (emitTo as (...a: unknown[]) => unknown)(...args),
}));

import { emitToOwner, listenOwn, ownerGroup, ownerOf, ownLabel } from "./windows";

beforeEach(() => {
  listen.mockClear();
  emitTo.mockClear();
});

test("ownerOf answers like the Rust label tests", () => {
  expect(ownerOf("main")).toBe("main");
  expect(ownerOf("diff")).toBe("main");
  expect(ownerOf("selection-history")).toBe("main");
  expect(ownerOf("merge-3")).toBe("main-3");
  expect(ownerOf("viewer-12")).toBe("main-12");
  for (const label of ["main-0", "main-1", "main-02", "diff-", "other", "selection-history-x", ""]) {
    expect(ownerOf(label)).toBeNull();
  }
});

test("ownerGroup lists the five labels of a project window", () => {
  expect(ownerGroup("main")).toEqual(["main", "diff", "merge", "viewer", "selection-history"]);
  expect(ownerGroup("main-2")).toEqual([
    "main-2",
    "diff-2",
    "merge-2",
    "viewer-2",
    "selection-history-2",
  ]);
});

test("ownLabel is the label of the current webview window", () => {
  expect(ownLabel()).toBe("diff-2");
});

test("listenOwn listens with the window's own target", () => {
  const handler = () => {};
  listenOwn("x", handler);
  expect(listen).toHaveBeenCalledWith("x", handler, { target: "diff-2" });
});

test("emitToOwner reaches every window of the project", async () => {
  await emitToOwner("y", 1);
  expect(emitTo.mock.calls).toEqual(
    ["main-2", "diff-2", "merge-2", "viewer-2", "selection-history-2"].map((label) => [
      label,
      "y",
      1,
    ]),
  );
});
