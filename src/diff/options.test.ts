import { beforeEach, expect, test } from "vitest";
import { loadOptions, loadWrap, storeOptions, storeWrap, type DiffPlace } from "./options";

const PLACES: DiffPlace[] = ["Default", "CommitDialog", "ChangesView", "Merge", "VcsLogView"];

beforeEach(() => localStorage.clear());

test("wrap_is_stored_per_place_with_its_default", () => {
  expect(PLACES.map(loadWrap)).toEqual([true, false, false, false, false]);

  storeWrap("CommitDialog", true);
  expect(PLACES.map(loadWrap)).toEqual([true, true, false, false, false]);
  expect(localStorage.getItem("diff.wrap.CommitDialog")).toBe("true");

  storeWrap("Default", false);
  expect(PLACES.map(loadWrap)).toEqual([false, true, false, false, false]);

  storeWrap("Merge", true);
  storeWrap("ChangesView", true);
  storeWrap("CommitDialog", false);
  expect(PLACES.map(loadWrap)).toEqual([false, false, true, true, false]);

  storeWrap("VcsLogView", true);
  expect(PLACES.map(loadWrap)).toEqual([false, false, true, true, true]);
  expect(localStorage.getItem("diff.wrap.VcsLogView")).toBe("true");
});

test("the_options_of_a_place_carry_its_wrap_and_the_shared_ones_do_not_store_it", () => {
  storeWrap("CommitDialog", true);

  expect(loadOptions().wrap).toBe(true);
  expect(loadOptions("CommitDialog").wrap).toBe(true);
  expect(loadOptions("ChangesView").wrap).toBe(false);

  storeOptions({ ...loadOptions("ChangesView"), align: true });

  expect(JSON.parse(localStorage.getItem("diff.options") ?? "{}")).not.toHaveProperty("wrap");
  expect(loadOptions("Default")).toMatchObject({ align: true, wrap: true });
});

test("without_storage_the_defaults_are_used", () => {
  const getItem = Storage.prototype.getItem;
  const setItem = Storage.prototype.setItem;
  Storage.prototype.getItem = () => {
    throw new Error("no storage");
  };
  Storage.prototype.setItem = () => {
    throw new Error("no storage");
  };
  try {
    expect(() => storeWrap("Merge", true)).not.toThrow();
    expect(PLACES.map(loadWrap)).toEqual([true, false, false, false, false]);
  } finally {
    Storage.prototype.getItem = getItem;
    Storage.prototype.setItem = setItem;
  }
});
