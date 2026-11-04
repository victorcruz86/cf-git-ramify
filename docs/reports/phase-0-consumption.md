# Phase 0: consumption report

Date: 2026-10-31. Raw data: `phase-0-consumption.json`.

## Result

| Goal | Target | App (median of 5) | IntelliJ 2023.2.3 | Met? |
|---|---|---|---|---|
| Memory at rest | About 300 MB | 224 MB, of which 102 MB is the WebView | 2,918 MB | Yes |
| CPU at rest | Close to 0 % | 0.03 % or less | 5 to 14 % | Yes |
| Startup until usable | Less than 1 s | 4.6 s (3.9 to 5.4) | Not measured | **No** |
| Log scroll without jumps | Less than 1 % of frames over 50 ms | 0.1 % (1 of 983), p99 of 19 ms | Not measured | Yes |

The app uses about 13 times less memory than IntelliJ with the same repository, which is what decides the result according to the design. Startup is the only goal that fails, and it remains to be solved before phase 1.

## Conditions

- Repository `git/git`: 85,936 commits and 1,022 refs. That is fewer than the 100,000 planned.
- App in release, one 1280×800 window, log loaded and a C diff open.
- Apple Silicon Mac. Memory with `footprint`, adding up the app process and the WebKit ones.
- CPU measured for 30 s, after 30 s of rest and with no file changes.
- Startup is not a strict cold start: the disk already had the data cached and it was not flushed. On a real start after rebooting it may take longer.
- The app was launched from a script, not from Finder.

## IntelliJ measurement

- IntelliJ IDEA CE 2023.2.3 with `git/git`, the Git window and a diff open, about 4 minutes after its start.
- Three readings: 2,919, 2,917 and 2,918 MB of memory, and 10 %, 5 % and 14 % of CPU.
- The memory includes its child processes and your installed plugins, because that is what you have open in your real use.
- CPU varies between readings, so it may not be real rest: it may have still been indexing. That is why it is given as a range. Memory was stable.
- Its startup and its scroll were not measured.

## Why startup fails

Sample from a single instrumented run. Times since launch:

| Moment | Time |
|---|---|
| Enters `run()` | 0.2 s |
| Tauri finishes starting (`setup`) | 1.3 s |
| The page starts in the WebView | 1.8 s |
| First rows drawn | 4.0 s |

Two other runs without that mark gave a page start between 2.3 and 2.6 s, and a log load of 1.3 s.

- **Log load (1.3 s or more):** loading the 85,936 commits before drawing. `git log` takes 0.9 s, and computing the graph only 0.05 s.
- **Native start (1.8 to 2.6 s):** happens before the page exists, unrelated to Git. It remains to be found out why Tauri takes 1.1 s to start and whether it is the same from Finder.

The first part has a known fix. The first 200 rows come out in 0.64 s with the same order, 0.10 s with a `commit-graph` and 0.03 s with only the current branch. It is possible to draw first and load the rest afterwards, or to keep the log on disk.

The second part is not resolved. Even if the log were instantaneous, startup would still be over 1 s with the current data.

## Other findings

- The graph generates 10.1 million segments: 118 on average per row and up to 280 lanes. A page of 200 rows takes 1.2 MB of JSON. Storing segments takes memory that the real graph of phase 2 should avoid by computing them on demand.
- The `commit-graph` does not speed up the full load (0.86 s versus 0.89 s).
- `requestAnimationFrame` stops if the window is covered, so the measurement mode hangs without it visible. It happened in a diagnostic run, not in the 5 of this table.
- The release binary size is 4.3 MB.

## Correction after the measurement

The final review found that the last 32 rows of the log could not be reached by scrolling because of a CSS error. It was fixed after measuring. It affects less than 0.04 % of the rows, so the 5 runs were not repeated.
