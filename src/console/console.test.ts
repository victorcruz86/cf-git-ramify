import { cleanup, render, screen } from "@testing-library/svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ConsoleLine } from "../lib/api";
import ConsoleView from "./ConsoleView.svelte";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}));

let send: (line: ConsoleLine) => void;

const COMMAND: ConsoleLine = {
  time: "10:42:07.123",
  command: true,
  text: "[project] git -c core.quotepath=false fetch origin",
};
const OUTPUT: ConsoleLine = { time: "10:42:08.001", command: false, text: "From /tmp/another" };

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "console_lines") return [COMMAND, OUTPUT];
    throw new Error(`unexpected command: ${String(command)}`);
  });
  vi.mocked(listen).mockImplementation(async (name, handler) => {
    if (name === "console-line") {
      send = (line) => (handler as (event: { payload: ConsoleLine }) => void)({ payload: line });
    }
    return () => {};
  });
});

afterEach(cleanup);

test("the_command_carries_the_time_and_the_output_does_not", async () => {
  render(ConsoleView, { active: true });

  expect(
    await screen.findByText("10:42:07.123: [project] git -c core.quotepath=false fetch origin"),
  ).toBeTruthy();
  expect(screen.getByText("From /tmp/another")).toBeTruthy();
});

test("new_lines_arrive_through_events", async () => {
  render(ConsoleView, { active: true });
  await screen.findByText("From /tmp/another");

  send({ time: "10:43:00.000", command: false, text: "Already up to date." });

  expect(await screen.findByText("Already up to date.")).toBeTruthy();
});

test("the_console_follows_the_end", async () => {
  const height = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(900);
  try {
    render(ConsoleView, { active: true });
    const log = (await screen.findByRole("log")) as HTMLElement;
    await vi.waitFor(() => expect(log.scrollTop).toBe(900));

    height.mockReturnValue(1200);
    send({ time: "10:43:00.000", command: false, text: "other" });

    await vi.waitFor(() => expect(log.scrollTop).toBe(1200));
  } finally {
    height.mockRestore();
  }
});

test("only_keeps_the_last_2000_lines", async () => {
  vi.mocked(invoke).mockResolvedValue(
    Array.from({ length: 2_000 }, (_, index) => ({ ...OUTPUT, text: `line ${index}` })),
  );
  render(ConsoleView, { active: true });
  await screen.findByText("line 1999");

  send({ ...OUTPUT, text: "the last" });

  await screen.findByText("the last");
  expect(screen.queryByText("line 0")).toBeNull();
  expect(screen.getByText("line 1")).toBeTruthy();
});
