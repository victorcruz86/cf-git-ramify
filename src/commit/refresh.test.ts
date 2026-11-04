import { expect, test, vi } from "vitest";
import { latestOnly } from "./refresh";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => (resolve = done));
  return { promise, resolve };
}

const tick = () => new Promise((resolve) => setTimeout(resolve));

test("applies_what_was_read", async () => {
  const apply = vi.fn();
  const run = latestOnly(async () => "a", apply, vi.fn());

  await run();

  expect(apply).toHaveBeenCalledExactlyOnceWith("a");
});

test("an_old_read_does_not_overwrite_the_new_one", async () => {
  const reads = [deferred<string>(), deferred<string>()];
  let next = 0;
  const apply = vi.fn();
  const run = latestOnly(() => reads[next++].promise, apply, vi.fn());

  const first = run();
  const second = run();
  await tick();
  expect(next).toBe(1);

  reads[0].resolve("old");
  await tick();
  expect(next).toBe(2);
  expect(apply).not.toHaveBeenCalled();

  reads[1].resolve("new");
  await Promise.all([first, second]);

  expect(apply).toHaveBeenCalledExactlyOnceWith("new");
});

test("many_requests_in_a_row_are_a_single_extra_read", async () => {
  const read = deferred<string>();
  const load = vi.fn(async () => {
    if (load.mock.calls.length === 1) return read.promise;
    return "end";
  });
  const run = latestOnly(load, vi.fn(), vi.fn());

  run();
  run();
  run();
  run();
  read.resolve("x");
  await tick();
  await tick();

  expect(load).toHaveBeenCalledTimes(2);
});

test("a_failure_is_reported_and_does_not_stop_the_next_ones", async () => {
  const apply = vi.fn();
  const fail = vi.fn();
  let attempts = 0;
  const run = latestOnly(
    async () => {
      if (attempts++ === 0) throw new Error("bad");
      return "good";
    },
    apply,
    fail,
  );

  await run();
  await run();

  expect(fail).toHaveBeenCalledTimes(1);
  expect(apply).toHaveBeenCalledExactlyOnceWith("good");
});

test("after_cancelling_nothing_is_applied", async () => {
  const read = deferred<string>();
  const apply = vi.fn();
  const run = latestOnly(() => read.promise, apply, vi.fn());

  const pending = run();
  run.cancel();
  read.resolve("late");
  await pending;

  expect(apply).not.toHaveBeenCalled();
});
