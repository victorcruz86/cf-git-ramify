import { expect, test } from "vitest";
import { PROJECT_PALETTE, projectColorIndex, projectInitials } from "./projectIcon";

// `AvatarUtils.initials`, which `RecentProjectIconHelper` uses through `AvatarIcon`.
test("a name of one word gives its first letter, in capitals", () => {
  expect(projectInitials("ramify")).toBe("R");
  expect(projectInitials("Ramify")).toBe("R");
});

test("a name with a separator gives the first letters of its first and last words", () => {
  expect(projectInitials("John Smith")).toBe("JS");
  expect(projectInitials("John-Smith-Harris")).toBe("JH");
  expect(projectInitials("cf-git-ide")).toBe("CI");
  expect(projectInitials("my_project")).toBe("MP");
  expect(projectInitials("my.project")).toBe("MP");
  expect(projectInitials("my,project")).toBe("MP");
});

test("a camelCase name gives its first letter and its first capital", () => {
  expect(projectInitials("MyProject")).toBe("MP");
  expect(projectInitials("myProject")).toBe("MP");
  expect(projectInitials("myBigProject")).toBe("MB");
});

test("a name with digits keeps them as letters of a word", () => {
  expect(projectInitials("project2")).toBe("P");
  expect(projectInitials("2fast")).toBe("F");
  expect(projectInitials("app-2")).toBe("A2");
  expect(projectInitials("web3Project")).toBe("WP");
});

test("a name without letters or digits gives nothing", () => {
  expect(projectInitials("")).toBe("");
  expect(projectInitials("---")).toBe("");
});

// `ColorPalette.select`: `abs(seed.hashCode()) % size`, with `String.hashCode` over 32 bits
// (`h = 31 * h + c` for each UTF-16 unit). By hand, "/a": '/' = 47 and 'a' = 97, so
// 47 * 31 + 97 = 1554, and 1554 % 9 = 6. "/ab": 1554 * 31 + 98 = 48272, and 48272 % 9 = 5.
// "/Users/me/work/ramify" overflows to -1439635813; abs is 1439635813 and 1439635813 % 9 = 7.
test("the colour index is the Java hash of the path modulo the nine gradients", () => {
  expect(projectColorIndex("/a")).toBe(6);
  expect(projectColorIndex("/ab")).toBe(5);
  expect(projectColorIndex("/Users/me/work/ramify")).toBe(7);
  expect(projectColorIndex("/home/dev/projects/app")).toBe(3);
  expect(projectColorIndex("/Users/victorcruz/PROJECTS/Experiments/cf-git-ide")).toBe(5);
});

test("the palette has the nine gradients of IntelliJ, each of two colours", () => {
  expect(PROJECT_PALETTE).toHaveLength(9);
  for (const [start, end] of PROJECT_PALETTE) {
    expect(start).toMatch(/^#[0-9A-F]{6}$/);
    expect(end).toMatch(/^#[0-9A-F]{6}$/);
  }
  expect(PROJECT_PALETTE[0]).toEqual(["#CE443C", "#E77E41"]);
  expect(PROJECT_PALETTE[8]).toEqual(["#D75370", "#E96FA3"]);
});
