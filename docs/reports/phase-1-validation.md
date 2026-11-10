# Phase 1: validation report

Date: 2026-11-01. Raw data: `phase-1-consumption.json` and `phase-1-large-diff.json`.

## Summary

- The feature of the phase is done in essence: Commit view, changelists, partial commit by lines, diff window, rollback with a copy and staging area mode.
- **The measurement is not complete.** During the whole session the screen was locked and off, so there was not a single frame. The first paint, the scroll, the real opening of a large file and all the checks that need an active window are missing.
- Of the 62 rows of phase 1, 35 are done, 8 are partial and 19 are pending. 9 of those rows also need a check by hand.
- Nothing was tested on real repositories or with a physical keyboard. The phase does not end until you complete the list in section 3.

## 1. Measurements

### What could be measured and what could not

`requestAnimationFrame` does not run with the screen locked, and the app does not draw the log without it. In addition, WebKit suspends the page about 7 s after it opens: in one test, a counter per second stopped at 6.8 s. That is why the measurement mode first looks at whether the page says it is visible (`document.visibilityState` or `document.hasFocus()`). A hidden page decides 1.5 s after being shown that there are no frames and measures what it can within that margin. A visible page waits for its first frame for up to 5 s, so as not to lose the measurement if the main thread is busy loading the log. Today the page declared itself hidden, and that is why the first path was taken.

The path with a visible window could not be run today: only unit tests cover it. If a run ends without frames, `window_visible` comes out false and it does not enter the median while any run is visible. `visible_runs` says how many were.

| Measurement | Target | Phase 0 | Phase 1 (today) | IntelliJ 2023.2.3 | Met? |
|---|---|---|---|---|---|
| Memory at rest | About 300 MB | 224 MB, 102 MB of the WebView | **206 MB, 86 MB of the WebView**, with nothing painted | 2,918 MB | Probably, but it is a minimum: see below |
| CPU at rest | Close to 0 % | 0.03 % or less | **0.0 %**, with the page suspended | 5 to 14 % | Not demonstrated: a suspended page does not consume |
| Startup until the first paint | Less than 1 s | 4.6 s, with the log | **Not measured.** Last valid measurement: 1.3 s with `open -n`, from an earlier version (`phase-1-startup.md`) | Not measured | **No**, according to that measurement |
| Startup until the window is shown, a minimum of the first paint | Less than 1 s | Not measured | **1.45 s** (from 1.21 to 1.51) | Not measured | **No:** it already exceeds 1 s before painting |
| Opening the diff of a normal file | There is no target | Not measured | **0.29 s** without painting | Not measured | Not applicable |
| Log scroll | Less than 1 % of frames over 50 ms | 0.1 % | **Not measured** | Not measured | Unknown |

Medians of 5 runs in release, with `git/git`, the log loaded, the Commit view and `builtin/commit.c` modified and open in the diff window.

The system load (1-minute `load average`) at the start of each run was 7.2, 5.8, 6.1, 5.9 and 5.1. It is lower than in the earlier startup measurements, which were 35 to 150.

**How to read the memory.** The 206 MB are not directly comparable with the 224 MB of phase 0: today nothing was painted, there was no scroll and the diff of a log commit was not opened. On the other hand, today there are two windows and four WebKit processes (GPU, network and two content ones). With the window visible the value will be higher. The difference with IntelliJ is so large (about 14 times) that it should not change the conclusion, although that is also yet to be confirmed.

**How to read the startup.** Showing the window happens before the first paint, so 1.45 s is a minimum of the latter: the goal of less than 1 s is still not met under these conditions. An earlier series, with almost the same code and a similar load, gave 1.09 s (from 0.95 to 1.54). I do not know what those 0.3 s of difference are due to, so the value is less firm than it looks. The 1.3 s of the startup report are from before adding Commit, diff and staging, and have not been repeated with the current code.

### Large file in the diff window

Throwaway repository with a file of 72,000 lines, 4.5 MB and 1,532 hunks, equal in lines and hunks to the one in task 10, which was 4.6 MB. Release, medians of 5 runs. Load: 5.6, 7.0, 5.4, 4.8 and 5.1.

| Stretch | Time |
|---|---|
| **Total until the editors are created** | **0.39 s** (from 0.38 to 0.53) |
| `worktree_sides`, the backend with Git and the hunks | 0.16 s |
| Create the CodeMirror editors | 0.06 s |
| Create the window, load the page and read the state | About 0.17 s |
| Lay out and paint | **Not measured** |

**The 0.39 s is not the time you will see.** CodeMirror lays out in `requestAnimationFrame`, and without frames that work did not happen. The figure of 5.9 s from task 10 split 2.2 s in `worktree_sides` in debug and about 3.7 s in the rest. Today the creation of editors only explains 0.06 s of that rest, so the most likely thing is that the missing time is in laying out and painting, although it is not demonstrated.

What is clear is that the backend in release is not the problem: `worktree_sides` drops from 2.2 s to 0.16 s. Nothing was optimized.

This is how the repository is created. Afterwards it is measured with `--repo` pointing at that folder:

```python
import os, subprocess, sys

repo = sys.argv[1]
os.makedirs(repo, exist_ok=True)
def git(*args):
    subprocess.run(["git", "-c", "user.name=measurement", "-c", "user.email=measurement@example.com", *args],
                   cwd=repo, check=True)
path = os.path.join(repo, "big.txt")
lines = [f"line {i:06d} of filler to measure the opening of the diff in one\n" for i in range(72000)]
git("init", "-q")
open(path, "w").write("".join(lines))
git("add", "big.txt")
git("commit", "-q", "-m", "base")
for i in range(23, 72000, 47):
    lines[i] = f"line {i:06d} CHANGED to measure the opening of the diff in other\n"
open(path, "w").write("".join(lines))
```

### What changed in the binary

The executable goes from 4.3 MB to 5.5 MB, and the `.app` takes up 5.3 MiB.

### To complete the measurement

With the screen unlocked and the window in view:

```bash
export PATH=$HOME/.nvm/versions/node/v22.23.4/bin:$PATH
python3 scripts/measure.py --app src-tauri/target/release/ramify \
  --repo ~/ramify-measure/git --out docs/reports/phase-1-consumption.json --runs 5
```

First a file in `~/ramify-measure/git` has to be modified and then restored with `git checkout -- <path>`. The `window_visible` and `visible_runs` fields of the JSON say whether the measurement was complete.

## 2. Parity matrix

Row-by-row detail in the Result column of `docs/parity-matrix.md`.

| Section | Phase 1 rows | `done` | `partial` | `pending` |
|---|---|---:|---:|---:|
| 2. Interface parts | 6 | 6 | 0 | 0 |
| 3. Git settings | 5 | 3 | 2 | 0 |
| 4. IntelliJ actions | 51 | 26 | 6 | 19 |
| **Total** | **62** | **35** | **8** | **19** |

Two other rows, `Git.Stage.Compare.Three.Versions` and `Git.Stage.ThreeSideDiff`, are marked `pending` although the classification rule takes them to phase 3. They are the three-way comparison of staging.

Nine rows need confirmation by hand: 8 of the `done` ones and 1 of the `partial` ones.

Of the 19 pending:

- **5 are really missing in staging:** `Git.Stage.Add.Tracked`, `Git.Stage.Add.No.Content`, `Git.Stage.ToggleIgnored`, `Git.Stage.Show.Staged` and `Git.Stage.Show.Local`.
- **2 are missing in other parts:** `Vcs.ReformatCommitMessage` and `ChangesView.RemoveDeleted`.
- **1 is a different decision:** `ChangesView.SingleClickPreview`, because the diff opens in its own window.
- **6 ask for a code editor, and the app does not have one:** `Vcs.EditSource`, `Vcs.Diff.ShowDiffInEditorTab` and the three change markers, plus `ChangesView.Edit`, which in addition does not apply to Git.
- **1 has no effect with one repository per window:** `ChangesView.GroupBy.Repository`.
- **4 depend on other phases:** `Git.Commit.And.Push.Executor` and the two of `Vcs.ShowDiffWithLocal` on phase 2, and `ChangesView.UnshelveSilently` on phase 4.

### Corrected shortcuts

`scripts/extract_actions.py` did not convert the shortcuts inherited from `$default`. Now it swaps control and meta like `MacOSDefaultKeymap.convertShortcutFromParent`, checked against IntelliJ's installed class. It also respects `remove="true"` and `use-shortcut-of`, which it did not handle either.

- `Git.New.Branch.In.Log` goes from ⌃⌥N to ⌘⌥N.
- `ShelveChanges.UnshelveWithDialog` goes from ⌃⇧U to ⌘⇧U.
- `Git.Branches` no longer has a shortcut: on Mac, IntelliJ removes it on purpose.
- 14 rows without a shortcut receive that of the action they share it with, like `Git.Commit.Stage` (⌘K) and `Vcs.EditSource` (⌘ DOWN, F4).
- The four rows with `Shift F6` become `⇧ F6`.

The errors of `⌃⌥Z` had already been corrected by hand. Sections 1 to 3 were not regenerated.

To repeat it, with the files of the tag `idea/232.10072.27` copied to a folder:

```bash
python3 scripts/extract_actions.py \
  --actions plugin.xml VcsActions.xml vcs-log.xml dvcs.xml \
  --keymaps '$default.xml' 'Mac OS X 10.5+.xml' \
  --bundles ActionsBundle.properties GitBundle.properties > table.md
```

The output is section 4 without the Result column. Do not replace the matrix with it: the shortcuts are compared and only those cells are changed.

One minor difference remains uncorrected: three `Vcs.Log.*` rows appear in phase 4 in the matrix, but the script's current rules take them to phase 2. It is not a shortcut error and it will be reviewed in phase 2.

## 3. Checklist to verify by hand

None of this could be checked automatically. Use a test repository first.

### Pass with synthetic events (2026-11-01)

Done by Claude with `Ramify.app` in release on the throwaway lab (`scripts/validation_lab.py`), with System Events keys and CoreGraphics mouse. It reads the window through its accessibility tree and checks the result on disk and in git. There were no screenshots: the process has no screen recording permission. **It does not replace the checkboxes below, which remain open:** it is neither a physical keyboard nor a real repository.

| Point | Result | Evidence |
|---|---|---|
| `⌘9` and `⌘0` | Good | The active tab changes and toggles when repeated |
| `⌘K` | Good | Focus goes to the message: what is typed reaches the field. It does not check unversioned files, same as `ChangesViewCommitWorkflowHandler.kt:130-148` |
| Initial state | Good | `Unversioned Files` collapsed and unchecked; the 5 modified ones, checked |
| `⌃Espacio` | **Does not reach the app** | macOS has it assigned to switching the input source (`symbolichotkeys` 60, active, with U.S. and Spanish - ISO). `Set Active Changelist` from the menu does work |
| Create and activate a list | Good | `changelists.json` |
| Save on switching window and app | Good | ` *` on `Your version` and text on disk only after moving to the main window, and also when moving to Finder |
| `⌘W` in the diff | Good | It closes and the text stays on disk |
| Live refresh | Good | Two hunks written from outside appear in the diff |
| Partial commit by hunks and by lines | Good | `git show`: only the chosen hunk in one commit, and in another `line one` and `line three`, with `line two` still on disk |
| `⌥⌘Z` in the Commit view | Good | Opens `Rollback Changes`; Escape closes without touching anything |
| `⌥⌘Z` in the diff | Good | It reverts the line and leaves a copy in `snapshots/` |
| `⌘,` | Good | Opens Settings; Escape closes |
| Double click | Good | With a `clickCount` 2 event it opens the diff |
| Dragging the divider | Good | The Commit view's goes from y=681 to y=561. Not tested: the diff's |
| `⌘Q` with unsaved text | Good | The text stays on disk and the process ends |

The user accepted as good, on looking at them in the window, the blue and grey selection, the focus ring and the dragging of the scroll bars; that leaves no trace that can be checked. The points with a trace (saving and commit by lines) were not seen in the lab after their answer, and that is why this pass did them. Not checked: colors and measures next to IntelliJ, and the diff's dividers.

Block 2, staging mode, with the same method and `git status` after each step: `+` and `-` with the real mouse, `⌥⌘A`, `⌥⌘Z` in Staged (takes out of the stage) and in Unstaged (reverts without a dialog, with a copy), `Stage All`, the commit with `Commit from staging` and turning the mode off: all good. Not checked: that the icons show up on hover.

Minor finding: the diff gutter's checkboxes are not in the accessibility tree, because CodeMirror hides the gutter. With the mouse they work.


- [ ] `⌘K` goes to the commit message and includes what is new.
- [ ] `⌘Q` saves the text typed in the diff and quits.
- [ ] `⌘W` closes the diff window and saves first.
- [ ] `⌘,` opens the settings.
- [ ] `⌥⌘Z` opens Rollback in the Commit view and reverts lines in the diff.
- [ ] `⌥⌘A` moves to the stage in staging mode. With changelists it does nothing, and that is a known difference.
- [ ] `⌘0` and `⌘9` switch tabs and toggle when repeated.
- [ ] `⌃Space` makes the list active. macOS may keep that shortcut for itself if you have several keyboard languages.

### Mouse and active window

- [ ] The `+` and `-` icons appear on hover in staging mode, and clicking them does what they say.
- [ ] The dividers between panes drag properly in the Commit view and in the diff.
- [x] The tree selection looks blue with the window active and grey without focus. The screenshots of task 9 were with the window inactive.
- [x] The blue focus ring and the dragging of the scroll bars look right in WebKit.
- [x] Type in the diff and click on the main window or on another app: the ` *` disappears and the text stays on disk. It has only been tested with the `blur` event fired from the page, with the screen locked, not with a real focus change.
- [ ] Colors, buttons and measures look like IntelliJ's when placed side by side. They were compared by eye against the code, not with screenshots.

### App and real repository

- [ ] Open `Ramify.app` with a double click from Finder. It has only been tested with the binary directly and with `open -n`.
- [ ] A hook that uses `node` or `pnpm` works when opened from Finder. Without the terminal's `PATH` it may not find them, and it has to be compared with IntelliJ.
- [ ] If you sign commits, make a commit with real GPG. It has only been tested with a fake `gpg`.
- [ ] Make a real partial commit, choosing lines, and check it with `git show`.
- [ ] Repeat the measurement of section 1 with the window visible.
- [ ] Open a large file in the diff and see how long it really takes.

### Live comparison with IntelliJ

Compare the commands of the same partial commit in a test repository. Close IntelliJ and open it from a terminal, so that it inherits the variables:

```bash
export RAMIFY_GIT_REAL=$(command -v git)
export RAMIFY_GIT_LOG=~/intellij-commands.log
"/Applications/IntelliJ IDEA CE.app/Contents/MacOS/idea"
```

Then set `scripts/git-wrapper.sh` as the path of the Git executable in its settings and make the commit.

- [ ] Compare the log with the table in `docs/reports/phase-1-commands.md`.
- [ ] Look at the commands marked "(unread)", above all the options of `add` and whether `rm` carries `-f`.
- [ ] Look at the order of the result when confirming lines of a hunk with a different number of lines on each side.
- [ ] Change `A,B,C` to `X,Y`, confirm only `X` and compare the order with IntelliJ. Ramify leaves `A X B C`.
- [ ] Afterwards put IntelliJ's Git path back to its previous value.

## 4. Known differences with IntelliJ

### They can lose work

- **Quit from the Dock and logout do not save on exit.** `⌘Q` does, because it goes through its own menu item. If the diff window had already lost focus, what was typed was saved then and nothing is lost. What is typed with that window still active on exit is still lost, and so is what could not be saved on losing focus, such as a file that changed on disk. It has not been tested whether the Dock menu or logout manage to take focus away from it. In IntelliJ nothing is lost.
- **With `autocrlf=input`, which is your real configuration, undoing lines converts a whole CRLF file to LF.** A recoverable copy remains.
- **Rollback has one moment left uncovered.** It compares size and date with the copy before starting, before `git checkout` and before deleting, and stops if something changed. What is written between the last check and that step, or without changing either size or date, is lost without a copy.

### They behave differently

- Rollback rejects files in conflict, instead of saving their stages as IntelliJ does. It is reviewed in phase 3.
- With staging area and a mixed selection, `⌥⌘Z` does revert and unstage at the same time. IntelliJ probably only does the first action.
- With changelists, `⌥⌘A` does nothing: what is new only enters through its checkbox when committing.
- Ignored files are never shown.
- The message history is saved only after a successful commit. IntelliJ saves it before.
- The CRLF detection is not the same as that of `GitCrlfProblemsDetector`. Fix and Commit does not warn if it fails.
- `prepare-commit-msg` does not count as a hook when deciding whether there are hooks.
- The checkbox of the rollback dialog is not remembered between openings.
- Stage in the diff does not save first, as IntelliJ does, and the margin clicks do nothing with unsaved text.
- Unstaging all the hunks of a rename leaves a deletion and a new file separately.
- A link that points outside the repository or is broken stops Stage All.
- A broken link does not open in the diff.
- Only UTF-8 files whose form stored by Git equals that of the disk are edited in the diff. With `autocrlf=input`, your real configuration, a file with CRLF on disk is read-only in the diff.
- A partial selection during a merge is not supported: leaving files out of the commit or choosing lines gives an error, and the merge has to be committed whole. What IntelliJ does has not been checked.
- When choosing only some lines of a multi-line replacement, what is chosen lands among the old lines: `A,B,C` changed to `X,Y` with only `X` chosen gives `A X B C`. Not compared with IntelliJ.
- Only the non-modal commit interface exists, and the diff always opens in its own window.
- The app has no code editor, so the actions that depend on it do not exist.
- Commit and Push comes out disabled until phase 2.
- The order of the settings is not that of version 2023.2.
- The log is still the provisional one from phase 0.

### Chosen by eye

They are not copied from IntelliJ's code: line height of 20 px, 30 px between columns, the style of the alignment gaps, the text `N unchanged lines`, the titles `HEAD` and `Your version` without a hash, and the system monospaced font.

## Conclusion

What was measured does not contradict the memory and CPU goals, but it does not demonstrate them either, and startup is still above 1 s. Measuring with a visible window is still missing, and so is your completing the list in section 3, above all the real partial commit and the live comparison. Until then, phase 1 is not validated.
