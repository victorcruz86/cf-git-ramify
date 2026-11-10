import { beforeEach, describe, expect, it } from "vitest";
import { migrateStorageKeys } from "./storageMigration";

describe("migrateStorageKeys", () => {
  beforeEach(() => localStorage.clear());

  it("copies rama's keys to Ramify's and keeps them", () => {
    localStorage.setItem("rama.annotation.options", '{"date":true}');
    localStorage.setItem("GitVcsSettings.SIGN_OFF_COMMIT:/r", "true");

    migrateStorageKeys(localStorage);

    expect(localStorage.getItem("ramify.annotation.options")).toBe('{"date":true}');
    expect(localStorage.getItem("rama.annotation.options")).toBe('{"date":true}');
    expect(localStorage.getItem("GitVcsSettings.SIGN_OFF_COMMIT:/r")).toBe("true");
  });

  it("never overwrites a key Ramify already saved", () => {
    localStorage.setItem("rama.compareBranches.swapSides", "true");
    localStorage.setItem("ramify.compareBranches.swapSides", "false");

    migrateStorageKeys(localStorage);

    expect(localStorage.getItem("ramify.compareBranches.swapSides")).toBe("false");
  });
});
