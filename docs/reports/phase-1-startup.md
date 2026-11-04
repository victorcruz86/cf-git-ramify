# Phase 1: startup report

Date: 2026-10-31. App in release, Apple Silicon Mac, repository `git/git`.

## Result

| Goal | Target | Phase 0 | Now | Met? |
|---|---|---|---|---|
| Startup until usable | Less than 1 s | 4.6 s, with the log | **1.3 s** with `open -n`, 1.2 s with the binary directly | **No** |

Median of the first paint of the Commit view, over 9 runs per mode. None of the 18 went below 1 s: the best was 1.05 s. The measurement was taken with the machine heavily loaded (see Conditions). In earlier series, with a load of about 35 to 50, the medians were 1.0 s with the binary directly and 1.1 s with `open -n`, so at rest the value may be around the target. This last point could not be checked.

The improvement comes from not loading the log on open. Hiding the window avoids seeing it blank, but does not shorten startup in a measurable way.

## Conditions

- System load (`load average`) of 35 to 150 during the measurements, mostly because of a virtual machine unrelated to the test. The noise is high: the ranges matter more than the medians.
- Alternated runs, 3 batches of 3 per variant and mode, to spread the load among them.
- First paint: first `requestAnimationFrame` after showing the window.
- The data on disk was already cached. It is not a cold start after rebooting.
- The native marks come from temporary instrumentation that is not in the commit. The `first-paint` mark does stay in the app and appears in `stages` of the measurement mode.

## Where the time goes

Medians in milliseconds, with the range in parentheses.

| Stretch | `open -n` | Binary directly |
|---|---|---|
| Until entering `run()` | 79 (65–119) | 10 (8–15) |
| `run()` to `setup` | 446 (365–582) | 414 (353–489) |
| `setup` to page start | 181 (166–692) | 152 (130–572) |
| Page start to end of `onMount` | 510 (59–551) | 486 (51–561) |
| End of `onMount` to first paint | 45 (35–56) | 135 (110–169) |
| **Total until first paint** | **1,323 (1,124–1,426)** | **1,188 (1,047–1,413)** |

Each stretch has its own median, so the sum does not give the total exactly.

Launching with `open -n` adds about 70 ms before `run()`, and the rest matches the binary directly. `open -n` uses LaunchServices, like Finder, although it was not tested with a real double click.

## Cause of the stretch between `run()` and `setup`

**The 1.1 s stretch of phase 0 was not reproduced: it now measures about 0.4 s.** That value came from a single run. The stretch varies a lot with the load: it reached 0.84 s in these series and more than 3 s in an earlier series with the machine more saturated.

That time belongs neither to Tauri nor to the app. Tauri creates the window and the WebView *before* the `setup` hook, so the stretch includes the start of AppKit and WebKit.

| Part of the stretch | Measured time | Active `sample` samples |
|---|---|---|
| Create window and `WKWebView` (inside `applicationDidFinishLaunching`) | About 290 ms together with the next row | 124 (55 %) |
| `-[NSApplication finishLaunching]` | Included in the previous row | 44 (20 %) |
| `Builder::build`, mostly `+[NSApplication sharedApplication]` | About 120 ms | 56 (25 %) |

`sample` slows the app down by 3 to 5 times, so its figures only serve as a proportion. The most expensive functions on the main thread are:

- `-[WKWebView initWithFrame:configuration:]`: starts the GPU process and queries the renderer with `CGLQueryRendererInfo`.
- `-[WKWebView loadRequest:]`: launches the WebContent process and registers the user fonts.
- `-[NSWindow initWithContentRect:…]`: creates the frame and the window buttons.
- `WebPreferences::platformInitializeStore`: waits for a response from `tccd`.
- `+[NSWritingToolsCoordinator isWritingToolsAvailable]`: AppKit loads the Writing Tools library with `dlopen` when preparing the Edit menu.

96 samples of `finishLaunching` were discarded. They are `dyld` waiting for `sample` (`notifyDebuggerLoad`), an effect of the measurement itself and not a real cost.

## Largest stretch left unresolved

About 400 ms pass between the page start and the start of the app's code. It is the longest stretch and it could not be shortened. What was measured:

- `index.html` and the resources are delivered in under 5 ms.
- An inline script runs at 3 ms, so the JavaScript engine is already ready.
- After the first line of `main.ts`, `onMount` finishes in about 30 ms, including `repo_open` with its two `git` calls.

The block is inside WebContent, before the script runs. Its exact cause remains unidentified: that process was not sampled because there are also other WebKit ones from other apps, and `sample` suspends its target.

## What was applied

- Hidden window (`"visible": false`) that is shown when `onMount` finishes, with background `#1e1f22` to avoid the white.
- Closing the window hides it and the app keeps running in the background. Reopening (`RunEvent::Reopen`) shows the same window without starting another process.
- `⌘Q` quits completely.

The hidden window brings the page start forward, but the time is recovered when showing it: the first paint does not change in a measurable way.

| Variant | `open -n` | Binary directly |
|---|---|---|
| Window visible from the start | 1,267 (1,128–2,012) | 1,229 (1,132–1,600) |
| Hidden window (the one applied) | 1,323 (1,124–1,426) | 1,188 (1,047–1,413) |

## What was tried and did not help

| Test | Result | Decision |
|---|---|---|
| Load `DiffView` and CodeMirror on demand (bundle from 453 to 51 KB) | End of `onMount` unchanged: 902 versus 946 ms | Discarded |
| Classic script with `defer` instead of a module | The 400 ms stay the same | Discarded |
| Remove the default macOS menu | `run()` to `setup` drops by 30 to 40 ms, first paint the same (1,200 ms) | Discarded: `⌘C`, `⌘V` and `⌘Q` would be lost |

## What is missing to get under 1 s

No clear lever remains in the app's code. Almost all the time is in AppKit and WebKit, and the longest stretch does not depend on the size of the code.

The next step is to repeat the measurement with the machine at rest and compare from Finder. If it is still above, the way forward is the diagnosis of WebContent. Reopening with the app in the background already avoids this cost: the window was visible 0.8 s after requesting it, which is an upper bound because that test did not measure in more detail.
