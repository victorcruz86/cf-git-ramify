import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import { get } from "svelte/store";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { dismiss, notifications, notify } from "./notifications";
import Notifications from "./Notifications.svelte";

beforeEach(() => vi.useFakeTimers());

afterEach(() => {
  cleanup();
  get(notifications).forEach(({ id }) => dismiss(id));
  vi.useRealTimers();
});

const titles = () => get(notifications).map((shown) => shown.title);

test("informational_notification_closes_on_its_own", async () => {
  render(Notifications);
  notify({ kind: "info", title: "Fetch successful" });
  expect(await screen.findByText("Fetch successful")).toBeTruthy();

  vi.advanceTimersByTime(9_999);
  expect(titles()).toEqual(["Fetch successful"]);

  vi.advanceTimersByTime(1);
  expect(titles()).toEqual([]);
  await vi.waitFor(() => expect(screen.queryByText("Fetch successful")).toBeNull());
});

test("sticky_notification_stays", () => {
  notify({ kind: "info", title: "Pull failed", sticky: true });

  vi.advanceTimersByTime(10 * 60_000);

  expect(titles()).toEqual(["Pull failed"]);
});

test("warnings_and_errors_stay_until_closed", () => {
  notify({ kind: "warning", title: "Warning" });
  notify({ kind: "error", title: "Error" });

  vi.advanceTimersByTime(10 * 60_000);

  expect(titles()).toEqual(["Warning", "Error"]);
});

test("action_button_closes_it", async () => {
  const run = vi.fn();
  render(Notifications);
  notify({ kind: "warning", title: "Push rejected", actions: [{ label: "Force Push", run }] });

  await fireEvent.click(await screen.findByRole("button", { name: "Force Push" }));

  expect(run).toHaveBeenCalledTimes(1);
  expect(titles()).toEqual([]);
});

// `.action` in controls.css is the toolbar's 26×26 icon button: a text link with it broke into
// one letter per line over the title ("Vie / w / Co / mmi / ts").
test("an_action_is_a_text_link_not_an_icon_button", async () => {
  render(Notifications);
  notify({ kind: "info", title: "12 files updated in 3 commits", actions: [{ label: "View Commits", run: () => {} }] });

  const link = await screen.findByRole("button", { name: "View Commits" });

  expect(link.classList.contains("action")).toBe(false);
  expect(link.classList.contains("link")).toBe(true);
});

test("the_cross_closes_the_notification_and_the_body_is_visible", async () => {
  render(Notifications);
  notify({ kind: "error", title: "Fetch failed", body: "origin: could not be resolved" });
  expect(await screen.findByText("origin: could not be resolved")).toBeTruthy();

  await fireEvent.click(screen.getByRole("button", { name: "Close" }));

  expect(titles()).toEqual([]);
});

test("each_notification_closes_separately", () => {
  const first = notify({ kind: "error", title: "One" });
  notify({ kind: "error", title: "Two" });

  dismiss(first);

  expect(titles()).toEqual(["Two"]);
});
