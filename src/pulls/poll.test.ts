import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { etagPoller, POLL_INTERVAL } from "./poll";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

function poller(answers: (string | null | Error)[]) {
  const check = vi.fn(async () => {
    const answer = answers.shift() ?? null;
    if (answer instanceof Error) throw answer;
    return answer;
  });
  const outdated = vi.fn();
  return { check, outdated, poll: etagPoller(check, outdated) };
}

test("the_list_is_outdated_only_after_a_changed_etag_and_polls_only_while_shown_and_focused", async () => {
  const { check, outdated, poll } = poller(["a", "a", "b", "b", null, "c"]);
  // Not running: nothing is asked.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 3);
  expect(check).not.toHaveBeenCalled();

  poll.update(true);
  // The first poll is 30 s after the start, and only sets the baseline.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL - 1);
  expect(check).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(check).toHaveBeenCalledTimes(1);
  expect(outdated.mock.calls).toEqual([[false]]);

  // The same ETag: not outdated. Another one: outdated.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls.at(-1)).toEqual([false]);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls.at(-1)).toEqual([true]);
  // As in 241, every answer decides again: the next one, unchanged, is not outdated.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls.at(-1)).toEqual([false]);
  // No ETag: not outdated, and it is the new baseline, so "c" only sets it again.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls.slice(-2)).toEqual([[false], [false]]);
  expect(check).toHaveBeenCalledTimes(6);

  // Hidden or unfocused: no more polls, and the baseline stays for when it comes back.
  poll.update(false);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 3);
  expect(check).toHaveBeenCalledTimes(6);
  poll.stop();
});

test("a_reload_resets_the_baseline_and_a_failure_is_ignored", async () => {
  const { check, outdated, poll } = poller(["a", new Error("offline"), "b", "c"]);
  poll.update(true);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  // A failure changes nothing, and polling goes on.
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls).toEqual([[false]]);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls.at(-1)).toEqual([true]);

  // A reload: not outdated, and the next answer is a baseline again.
  poll.reset();
  expect(outdated.mock.calls.at(-1)).toEqual([false]);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(outdated.mock.calls.at(-1)).toEqual([false]);
  expect(check).toHaveBeenCalledTimes(4);
  poll.stop();
});

test("an_answer_that_arrives_after_a_reset_or_a_stop_is_dropped", async () => {
  let release: (etag: string) => void = () => {};
  const check = vi.fn()
    .mockResolvedValueOnce("a")
    .mockImplementationOnce(() => new Promise<string>((resolve) => (release = resolve)));
  const outdated = vi.fn();
  const poll = etagPoller(check, outdated);
  poll.update(true);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
  expect(check).toHaveBeenCalledTimes(2);
  poll.reset();
  release("b");
  await vi.advanceTimersByTimeAsync(0);
  // "b" was asked before the reset: it is neither outdated nor the new baseline.
  expect(outdated.mock.calls).toEqual([[false], [false]]);
  poll.stop();
});

// GitHub refused the token: every poll would read the Keychain again (a prompt on an unsigned
// build) to be refused again. The poll stops, and starts again only when it is turned on again,
// which the list does once a re-login has reloaded it.
test("a_refusal_stops_the_poll_until_it_is_turned_on_again", async () => {
  const refusal = { kind: "authentication", message: "Request response: Bad credentials." };
  const check = vi.fn().mockResolvedValueOnce("a").mockRejectedValueOnce(refusal).mockResolvedValue("b");
  const stops = vi.fn((error: unknown) => error === refusal);
  const poll = etagPoller(check, vi.fn(), POLL_INTERVAL, stops);
  poll.update(true);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 2);
  expect(check).toHaveBeenCalledTimes(2);
  expect(stops).toHaveBeenCalledWith(refusal);

  await vi.advanceTimersByTimeAsync(POLL_INTERVAL * 5);
  expect(check).toHaveBeenCalledTimes(2);

  poll.update(false);
  poll.update(true);
  await vi.advanceTimersByTimeAsync(POLL_INTERVAL);
  expect(check).toHaveBeenCalledTimes(3);
  poll.stop();
});
