// GHPRListETagUpdateChecker: the list's ETag every 30 s with a fixed delay, the first time 30 s
// after the start. Every answer decides `outdated` again: it is outdated when there was an ETag
// before and the new one differs (`current != null && value != null && current != value`), so the
// first answer only sets the baseline. A failure is ignored, unless `stops` says it stops the poll:
// GitHub refused the token, and every poll would read it from the Keychain again (a prompt on an
// unsigned build) to be refused again; the poll then waits for the next `update(true)`. Overview
// decision 16: it polls only while `update(true)`, which the list calls while it is shown and the
// app has the focus; 241 polls while the connection exists.

export const POLL_INTERVAL = 30_000;

export function etagPoller(
  check: () => Promise<string | null>,
  onoutdated: (outdated: boolean) => void,
  interval: number = POLL_INTERVAL,
  stops: (error: unknown) => boolean = () => false,
) {
  let last: string | null = null;
  let running = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  // Bumped by a reset or a stop: an answer asked before it is dropped.
  let epoch = 0;

  const clear = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const schedule = () => {
    clear();
    if (running) timer = setTimeout(() => void tick(), interval);
  };

  async function tick() {
    timer = null;
    const asked = epoch;
    let etag: string | null;
    try {
      etag = await check();
    } catch (error) {
      if (asked !== epoch) return;
      if (stops(error)) running = false;
      else schedule();
      return;
    }
    if (asked !== epoch) return;
    onoutdated(last !== null && etag !== null && last !== etag);
    last = etag;
    schedule();
  }

  return {
    // Shown and focused (`true`), or not: the timer starts again, or stops and keeps the baseline.
    update(now: boolean) {
      if (now === running) return;
      running = now;
      epoch++;
      schedule();
    },
    // `stop`: a reload forgets the baseline, and the list is no longer outdated.
    reset() {
      epoch++;
      last = null;
      onoutdated(false);
      schedule();
    },
    stop() {
      running = false;
      epoch++;
      clear();
    },
  };
}
