import { expect, test } from "vitest";
import { TOKENS } from "../lib/theme";
import { graphColor } from "./graphColor";

const MASTER_HASH = -1081267614;
const ORIGIN_MASTER_HASH = 468329835;

test("palette_same_as_intellij", () => {
  expect(graphColor(1)).toBe("rgb(100,150,160)");
  expect(graphColor(0)).toBe(TOKENS.text);
  expect(graphColor(MASTER_HASH)).toBe("rgb(140,168,118)");
  expect(graphColor(ORIGIN_MASTER_HASH)).toBe("rgb(152,126,80)");
});

test("channels_within_range_with_extreme_ids", () => {
  for (const id of [2147483647, -2147483648, -1]) {
    const channels = graphColor(id).match(/\d+/g)!.map(Number);
    expect(channels).toHaveLength(3);
    for (const channel of channels) {
      expect(channel).toBeGreaterThanOrEqual(70);
      expect(channel).toBeLessThanOrEqual(169);
    }
  }
});

test("overflow_in_32_bits_keeps_the_sign_of_the_remainder", () => {
  // 2147483647 * 200 + 30 gives -170 in 32 bits, and abs(-170 % 100) + 70 = 140.
  expect(graphColor(2147483647)).toBe("rgb(140,150,80)");
});
