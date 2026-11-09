import { expect, test } from "vitest";
import { formatFileSize, tabTitle, withoutTab, withTarget, type ViewerTab } from "./model";

const HASH = "1234567890abcdef1234567890abcdef12345678";

// `StringUtilRtTest`-like cases of `formatFileSize`: metric units, at most two decimals.
test("file_sizes_are_formatted_as_string_util_rt", () => {
  expect(formatFileSize(0)).toBe("0 B");
  expect(formatFileSize(1)).toBe("1 B");
  expect(formatFileSize(999)).toBe("999 B");
  expect(formatFileSize(1000)).toBe("1 kB");
  expect(formatFileSize(1234)).toBe("1.23 kB");
  expect(formatFileSize(2_560_000)).toBe("2.56 MB");
  expect(formatFileSize(31_457_280)).toBe("31.46 MB");
  expect(formatFileSize(20_480_000)).toBe("20.48 MB");
  expect(formatFileSize(999_995)).toBe("1 MB");
  // `HALF_EVEN`: an exact half goes to the even neighbour.
  expect(formatFileSize(2125)).toBe("2.12 kB");
  expect(formatFileSize(2375)).toBe("2.38 kB");
  // `DecimalFormat` rounds the exact binary value: 2.135 is 2.13499…
  expect(formatFileSize(2135)).toBe("2.13 kB");
});

test("titles_name_the_file_and_the_short_revision", () => {
  expect(tabTitle({ path: "src/a.txt", revision: null })).toBe("a.txt");
  expect(tabTitle({ path: "src/a.txt", revision: HASH })).toBe("a.txt (12345678)");
  expect(tabTitle({ path: "a.txt", revision: HASH, failed: true })).toBe("a.txt (0)");
});

test("closing_the_active_tab_activates_the_one_on_its_left", () => {
  let tabs: ViewerTab[] = [];
  let active: string | null = null;
  for (const path of ["a", "b", "c"]) {
    ({ tabs, key: active } = withTarget(tabs, active, { path, revision: null }));
  }
  const keys = tabs.map((tab) => tab.key);

  expect(withoutTab(tabs, keys[2], keys[2]).active).toBe(keys[1]);
  expect(withoutTab(tabs, keys[0], keys[0]).active).toBe(keys[1]);
  expect(withoutTab(tabs, keys[2], keys[0]).active).toBe(keys[2]);
  expect(withoutTab(tabs.slice(0, 1), keys[0], keys[0])).toEqual({ tabs: [], active: null });
});
