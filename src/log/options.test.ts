import { expect, test } from "vitest";
import { defaultOptions, readOptions } from "./options";

test("bad_stored_widths_are_dropped", () => {
  const options = readOptions({ columnWidths: { author: "120", date: -1, hash: 70, subject: 300 } });
  expect(options.columnWidths).toEqual({ hash: 70 });
});

test("options_without_widths_read_as_none", () => {
  expect(readOptions({ sort: "linear" }).columnWidths).toEqual({});
  expect(readOptions({ columnWidths: "wide" }).columnWidths).toEqual({});
  expect(readOptions(null)).toEqual(defaultOptions());
});
