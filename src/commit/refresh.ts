export interface Refresh {
  (): Promise<void>;
  cancel(): void;
}

// Reads go one at a time, so they cannot arrive out of order. A request that arrives during a read
// makes the response in flight stale: it is not applied and the data is read again, and several
// requests in a row end up as a single extra read.
export function latestOnly<T>(
  load: () => Promise<T>,
  apply: (value: T) => void,
  fail: (failure: unknown) => void,
): Refresh {
  let running: Promise<void> | null = null;
  let again = false;
  let cancelled = false;

  async function loop() {
    do {
      again = false;
      try {
        const value = await load();
        if (!again && !cancelled) apply(value);
      } catch (failure) {
        if (!again && !cancelled) fail(failure);
      }
    } while (again && !cancelled);
    running = null;
  }

  const run: Refresh = () => {
    if (cancelled) return Promise.resolve();
    if (running) {
      again = true;
      return running;
    }
    running = loop();
    return running;
  };
  run.cancel = () => {
    cancelled = true;
  };
  return run;
}
