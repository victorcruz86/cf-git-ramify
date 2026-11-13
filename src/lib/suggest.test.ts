import { afterEach, expect, test } from "vitest";
import { suggestWhileTyping } from "./suggest";

afterEach(() => document.body.replaceChildren());

function field() {
  const input = document.createElement("input");
  document.body.append(input);
  return input;
}

test("a_field_has_no_suggestions_until_something_is_typed", () => {
  const input = field();
  suggestWhileTyping(input, "branches");

  expect(input.getAttribute("list")).toBeNull();
  expect(input.getAttribute("role")).toBe("combobox");

  input.dispatchEvent(new Event("beforeinput"));
  expect(input.getAttribute("list")).toBe("branches");
});

test("leaving_the_field_closes_the_suggestions_until_the_next_keystroke", () => {
  const input = field();
  suggestWhileTyping(input, "branches");
  input.dispatchEvent(new Event("input"));

  input.dispatchEvent(new Event("blur"));
  expect(input.getAttribute("list")).toBeNull();

  input.dispatchEvent(new Event("input"));
  expect(input.getAttribute("list")).toBe("branches");
});

test("destroying_it_stops_listening", () => {
  const input = field();
  const action = suggestWhileTyping(input, "branches");
  action.destroy();

  input.dispatchEvent(new Event("input"));
  expect(input.getAttribute("list")).toBeNull();
});
