import { expect, test } from "vitest";
import { isBare, ticketOf, ticketPrefix } from "./ticket";

test("the_ticket_is_the_first_key_and_number_in_the_branch_name", () => {
  expect(ticketOf("bugfix/HT-730-partner-team-removal-error-message")).toBe("HT-730");
  expect(ticketOf("HT-12")).toBe("HT-12");
  expect(ticketOf("feature/ABC2-7_and-HT-9")).toBe("ABC2-7");
  expect(ticketPrefix("bugfix/HT-730-x")).toBe("HT-730 ");
});

test("no_ticket_without_an_upper_case_key_and_a_number", () => {
  for (const branch of ["main", "feature/ht-730-lower", "release-2024", "HT-", "X-1a", "fooHT-7"]) {
    expect(ticketOf(branch)).toBeNull();
  }
  expect(ticketOf(null)).toBeNull();
  expect(ticketPrefix(null)).toBe("");
});

test("a_message_with_only_the_ticket_is_bare", () => {
  expect(isBare("", null)).toBe(true);
  expect(isBare("  \n", "HT-730")).toBe(true);
  expect(isBare("HT-730 ", "HT-730")).toBe(true);
  expect(isBare(" HT-730\n", "HT-730")).toBe(true);
  expect(isBare("HT-730 Fix it", "HT-730")).toBe(false);
  expect(isBare("HT-730 ", null)).toBe(false);
});
