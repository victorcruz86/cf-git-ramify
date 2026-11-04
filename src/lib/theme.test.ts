import { expect, test } from "vitest";
import { applyTheme, TOKENS } from "./theme";

test("the_values_come_from_the_dark_theme", () => {
  expect(TOKENS["panel"]).toBe("#2B2D30");
  expect(TOKENS["editor"]).toBe("#1E1F22");
  expect(TOKENS["selection"]).toBe("#2E436E");
  expect(TOKENS["selection-inactive"]).toBe("#43454A");
  expect(TOKENS["status-modified"]).toBe("#6897BB");
  expect(TOKENS["status-unversioned"]).toBe("#D1675A");
  expect(TOKENS["log-merge"]).toBe("#6F737A");
});

test("every_token_is_a_css_variable", () => {
  const root = document.createElement("div");

  applyTheme(root);

  for (const [name, value] of Object.entries(TOKENS)) {
    expect(root.style.getPropertyValue(`--${name}`)).toBe(value);
  }
});
