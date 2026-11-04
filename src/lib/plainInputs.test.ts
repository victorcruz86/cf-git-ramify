import { afterEach, beforeEach, expect, test } from "vitest";
import { plainInputs } from "./plainInputs";

let stop: () => void;

beforeEach(() => {
  stop = plainInputs(document);
});

afterEach(() => {
  stop();
  document.body.replaceChildren();
});

function focused(tag: "input" | "textarea") {
  const field = document.body.appendChild(document.createElement(tag));
  field.focus();
  return field;
}

test("a_new_field_has_no_macos_substitutions_when_it_gets_focus", () => {
  for (const field of [focused("input"), focused("textarea")]) {
    expect(field.getAttribute("spellcheck")).toBe("false");
    expect(field.getAttribute("autocorrect")).toBe("off");
    expect(field.getAttribute("autocapitalize")).toBe("off");
  }
});

test("what_is_not_a_text_field_is_not_touched", () => {
  const button = document.body.appendChild(document.createElement("button"));
  button.focus();

  expect(button.hasAttribute("spellcheck")).toBe(false);
});
