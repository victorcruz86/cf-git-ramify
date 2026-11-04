# Phase 0: consumption test and parity matrix. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show with data that a log and a diff in Tauri meet the consumption goals, and leave the parity matrix with IntelliJ 2023.2.3 approved.

**Architecture:** A Rust core that launches the `git` binary, loads the whole log into compact memory and computes the graph. The Svelte interface requests only the visible rows, draws them on a canvas and shows diffs with CodeMirror 6. A script measures memory, CPU, startup and smoothness of the real app.

**Tech Stack:** Tauri 2.12, Rust 1.99.1, Svelte 5, TypeScript, Vite 8, Vitest 5, CodeMirror 6 with `@codemirror/merge`, Python 3 for the measurement.

**Spec:** `docs/superpowers/specs/2026-10-31-intellij-git-client-design.md`

**Scope of this plan:** phase 0 only. Phases 1 to 4 will have their own plan, because the result of this phase may change the technology. The log and diff code of this phase is a technical test: what is kept is decided when planning phase 1.

**Decisions of this plan** (the design leaves them to whoever builds):
- Internal name of the app: `rama`.
- Interface in Svelte 5, for its low runtime consumption.
- Log drawn on a canvas with virtual rows.
- Log index in memory only in this phase.

## Global Constraints

- Platform: macOS arm64. No Windows or Linux support.
- Every Git operation uses the real `git` binary. Minimum version: 2.38.0.
- No JVM and no bundled Chromium.
- Automated tests use throwaway repositories, never real ones.
- The app does not use the name or logos of JetBrains or IntelliJ.
- Behaviour reference: IntelliJ IDEA CE 2023.2.3, build `IC-232.10072.27`, tag `idea/232.10072.27`. Do not use `master`.
- Goals (a guide, not a yes-or-no limit): about 300 MB of memory at rest, CPU close to 0 % at rest, less than 1 second cold startup, log scroll without visible jumps.
- Measurement conditions: one window, a public repository of about 100,000 commits, log loaded and a diff open, sum of `footprint` over all the app's processes including the WebView ones.

## Review Focus

1. Repository without commits: the log appears empty, with no error. Test in Task 3.
2. Shallow clone, with parents that do not exist: the graph is drawn without failing. Test in Task 3.
3. Messages or authors with bytes that are not UTF-8, and paths with spaces or accents: they are shown without losing the row. Tests in Tasks 3 and 6.
4. Binary or very large file in a diff: a notice is shown and the interface does not block. Test in Task 6.
5. `git` not installed, or a folder that is not a repository: clear message, the app does not close. Test in Task 2.

---

### Task 1: App skeleton

**Files:**
- Create: `.nvmrc`, `rust-toolchain.toml`, `.gitignore`, `package.json`, `vite.config.ts`, `tsconfig.json`, `index.html`, `src/main.ts`, `src/App.svelte`, `src/App.test.ts`
- Create: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/build.rs`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: `rama_lib::run()`, Tauri command `app_version() -> String`, and `pnpm test` and `cargo test` working.

- [ ] **Step 1: Initialize the repository and pin versions**

Run `git init -b main`. Create `.nvmrc` with `22.23.4` and `rust-toolchain.toml` with `channel = "1.99.1"`. Run `nvm use`.

Do not change the system's default Rust: other projects of the user may depend on it.

- [ ] **Step 2: Generate the project**

Create the project with Tauri 2 and the Svelte with TypeScript template. Identifier: `dev.rama.app`, window title `rama`, window of 1280×800. Add `vitest` and `@testing-library/svelte`.

- [ ] **Step 3: Write the failing test**

In `src/App.test.ts`, `shows the version`: mounts `App` with a mocked `invoke` that returns `"0.0.1"` and checks that the text `0.0.1` appears on screen.

- [ ] **Step 4: Check that it fails**

Run: `pnpm vitest run src/App.test.ts`
Expected: FAIL, the text is not found.

- [ ] **Step 5: Implement `app_version` and show it in `App.svelte`**

- [ ] **Step 6: Check the whole**

Run: `pnpm vitest run && (cd src-tauri && cargo test) && pnpm tauri build --debug --no-bundle`
Expected: everything green and binary created.

Also check that `pnpm tauri dev` opens the window and shows the version.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: app skeleton with Tauri and Svelte"
```

### Task 2: Running `git`

**Files:**
- Create: `src-tauri/src/git.rs`, `src-tauri/src/test_support.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum GitError { NotInstalled, NotARepository { path: String }, Failed { args: Vec<String>, code: Option<i32>, stderr: String }, Io(String) }`
  - `git::run(repo: &Path, args: &[&str]) -> Result<Vec<u8>, GitError>`
  - `git::run_with_binary(binary: &Path, repo: &Path, args: &[&str]) -> Result<Vec<u8>, GitError>`
  - `git::stream(repo: &Path, args: &[&str], delimiter: u8, on_record: impl FnMut(&[u8])) -> Result<(), GitError>`
  - `test_support::TempRepo::new()`, `.commit(file, contents, message) -> String`, `.path()`, `.git(args)`, only under `cfg(test)`

- [ ] **Step 1: Write the failing tests**

In `git.rs`:
- `run_returns_output`: `git rev-parse --is-inside-work-tree` in a `TempRepo` returns `true\n`.
- `folder_without_repository`: `run` in an empty temporary folder returns `NotARepository`, with the path.
- `missing_git_binary`: with `git::run_with_binary(Path::new("/does/not/exist/git"), repo, args)` it returns `NotInstalled`.
- `failed_command`: `git log --nonexistent-branch` returns `Failed` with non-empty `stderr`.
- `stream_splits_records`: with 3 commits, `log -z --format=%s` delivers 3 records.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test git::)`
Expected: FAIL, the functions do not exist.

- [ ] **Step 3: Implement `git.rs`**

`run` delegates to `run_with_binary` with `git` from the `PATH`. Set `LC_ALL=C` and `GIT_OPTIONAL_LOCKS=0`, so that reading does not block `index.lock` while Devin edits. `stream` reads stdout in blocks and must drain stderr in another thread to avoid a deadlock with long output. `NotARepository` is detected by `not a git repository` in stderr.

An app launched from Finder does not inherit the terminal's `PATH`, and `git` is in `/opt/homebrew/bin`. Look in the `PATH`, then in `/opt/homebrew/bin/git` and finally in `/usr/bin/git`.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test git::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: running the git binary with typed errors"
```

### Task 3: Loading the log

**Files:**
- Create: `src-tauri/src/log/mod.rs`, `src-tauri/src/log/parse.rs`, `src-tauri/src/log/store.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `git::stream`, `GitError`, `TempRepo`.
- Produces:
  - `struct Commit { hash: Box<str>, parents: Box<[u32]>, author: Box<str>, time: i64, refs: Box<str>, subject: Box<str> }`
  - `parse::parse_record(record: &[u8]) -> Option<RawCommit>`
  - `store::load(repo: &Path) -> Result<CommitStore, GitError>`
  - `CommitStore::len()`, `.get(i: u32) -> &Commit`, `.parents() -> impl Iterator<Item = &[u32]>`

`parents` are indices within the store itself. The order is from children to parents.

- [ ] **Step 1: Write the failing tests**

In `parse.rs`:
- `complete_record`: extracts the six fields.
- `subject_with_separator`: a subject with `\x1f` keeps the whole text.
- `non_utf8_bytes`: a subject with `\xff` is returned with a replacement character and without discarding the record.

In `store.rs`:
- `repository_without_commits`: `load` on a new `git init` gives `len() == 0`, not an error.
- `merge_resolves_parents`: a merge has two valid indices in `parents`.
- `shallow_clone`: `git clone --depth 2 file://<path>` of a `TempRepo` of 5 commits loads 2 commits and discards the missing parents. The `file://` URL is necessary: with a normal path, Git ignores `--depth`.
- `detached_head`: after `checkout --detach`, all the commits still load.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test log::)`
Expected: FAIL, the modules are empty.

- [ ] **Step 3: Implement the loading**

Launch `git log --all --topo-order -z --format=%H%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s`, with `HEAD` added to include a detached HEAD. A repository without commits fails with `does not have any commits yet` or without refs: turn it into an empty store.

The hash-to-index map is used only during loading and is freed when it finishes, so that it does not count in memory at rest.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test log::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: loading the whole log into memory"
```

### Task 4: Log graph

**Files:**
- Create: `src-tauri/src/log/graph.rs`
- Modify: `src-tauri/src/log/mod.rs`

**Interfaces:**
- Consumes: `CommitStore::parents`.
- Produces:
  - `struct Edge { from_lane: u16, to_lane: u16, color: u16 }`
  - `struct Row { lane: u16, color: u16, edges: Box<[Edge]> }`
  - `graph::layout<'a>(parents: impl Iterator<Item = &'a [u32]>) -> Vec<Row>`

- [ ] **Step 1: Write the failing tests**

With lists of parents per row:
- `lineal`: `[[1],[2],[]]` gives lanes `0,0,0`.
- `branch_and_merge`: `[[1,2],[3],[3],[]]` gives lanes `0,0,1,0`, and in row 2 there is an edge from lane 1 to 0.
- `octopus`: `[[1,2,3],[],[],[]]` gives lanes `0,0,1,2`.
- `raices_multiples`: `[[],[]]` reuses lane 0.
- `free_lane_is_reused`: a closed branch leaves its lane free for the next one.

- [ ] **Step 2: Check that they fail**

Run: `(cd src-tauri && cargo test graph::)`
Expected: FAIL, `layout` does not exist.

- [ ] **Step 3: Implement `layout`**

Keep `active: Vec<Option<(u32, u16)>>`, one lane per position with the commit it waits for and its color. For each row `i`:
1. The commit takes the first lane waiting for `i`, or the first free lane if there is none.
2. The other lanes waiting for `i` converge into it and become free.
3. The first parent continues in its lane.
4. Each additional parent uses a lane that already waits for it, or the first free lane with a new color.
5. Lanes that do not take part produce a straight edge.

Cost O(lanes) per row. This layout is provisional: phase 2 replaces it with the one from `platform/vcs-log/graph`.

- [ ] **Step 4: Check that they pass**

Run: `(cd src-tauri && cargo test graph::)`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: log graph layout"
```

### Task 5: Log view

**Files:**
- Create: `src-tauri/src/commands.rs`, `src/lib/api.ts`, `src/log/rowCache.ts`, `src/log/rowCache.test.ts`, `src/log/draw.ts`, `src/log/LogView.svelte`
- Modify: `src-tauri/src/lib.rs`, `src/App.svelte`

**Interfaces:**
- Consumes: `store::load`, `graph::layout`, `GitError`.
- Produces:
  - Command `log_open(repo: String) -> Result<LogSummary, ApiError>`, with `LogSummary { count: u32, max_lanes: u16 }`
  - Command `log_rows(start: u32, count: u32) -> Result<Vec<LogRow>, ApiError>`, with `LogRow { hash, subject, author, time, refs, lane, color, edges }`
  - `ApiError { kind: "git_not_found" | "not_a_repository" | "failed", message: String }`
  - `AppState { repo: Mutex<Option<OpenRepo>> }`
  - TypeScript: `visibleRange(scrollTop, viewportHeight, rowHeight, total, overscan) -> { start, end }` and `class RowCache { constructor(fetchPage, pageSize = 200, maxPages = 20); ensure(start, end): Promise<void>; get(index): LogRow | undefined }`
  - `LogView` component event: `onselect(hash: string)`

- [ ] **Step 1: Write the failing tests**

In `src/log/rowCache.test.ts`:
- `range_at_the_start`: `visibleRange(0, 260, 26, 1000, 5)` gives `{ start: 0, end: 15 }`.
- `range_at_the_end`: never exceeds `total`.
- `does_not_repeat_requests`: two simultaneous `ensure` calls for the same page call `fetchPage` once.
- `limits_pages`: when 25 pages are requested, only 20 stay in the cache.

In `commands.rs`:
- `log_rows_out_of_range`: `start` greater than `count` returns an empty list.
- `repository_error`: `log_open` on a folder without a repository returns `kind == "not_a_repository"`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test commands::)`
Expected: FAIL

- [ ] **Step 3: Implement commands, cache and canvas**

`log_open` loads in a `tauri::async_runtime::spawn_blocking` thread, so as not to freeze the window. The repository path arrives as `argv[1]`; if it is missing, the interface shows a text field to type it. The `ApiError` errors are shown in the window.

`LogView` draws on a single `<canvas>` of the visible size, inside a scrolling element of height `count × 26 px`. Rows of 26 px and pixel density according to `devicePixelRatio`. The colors are provisional, in a dark theme. A click on a row calls `onselect`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run src/log && (cd src-tauri && cargo test commands::)`
Expected: PASS

- [ ] **Step 5: Check in the app**

Run: `pnpm tauri dev`, and type the path of a repository in the text field.
Expected: the log is seen with its graph, scrolling works from start to end and a folder without a repository shows the error without closing the app.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: log view with virtual rows on a canvas"
```

### Task 6: Diff view

**Files:**
- Create: `src-tauri/src/diff.rs`, `src/diff/DiffView.svelte`, `src/diff/sides.ts`, `src/diff/sides.test.ts`
- Modify: `src-tauri/src/commands.rs`, `src/lib/api.ts`, `src/App.svelte`

**Interfaces:**
- Consumes: `git::run`, `LogView.onselect`, `AppState`.
- Produces:
  - `MAX_DIFF_BYTES: u64 = 5 * 1024 * 1024`
  - `enum Side { Text { content: String }, Binary, TooLarge { bytes: u64 }, Absent }`
  - Command `commit_files(hash: String) -> Result<Vec<ChangedFile>, ApiError>`, with `ChangedFile { path, old_path: Option<String>, status: String }`
  - Command `file_sides(hash: String, path: String, old_path: Option<String>) -> Result<FileSides, ApiError>`, with `FileSides { old: Side, new: Side }`
  - TypeScript: `sideMessage(side: Side): string | null`

- [ ] **Step 1: Write the failing tests**

In `diff.rs`:
- `modified_file`: both sides are `Text`.
- `new_file`: `old` is `Absent`.
- `deleted_file`: `new` is `Absent`.
- `binary_file`: content with a NUL byte gives `Binary`.
- `large_file`: 6 MiB gives `TooLarge { bytes: 6291456 }` without reading the whole content.
- `path_with_spaces_and_accents`: `folder with space/añó.txt` appears unescaped in `commit_files` and opens in `file_sides`.
- `initial_commit`: `commit_files` works without a parent.

In `sides.test.ts`, `sideMessage` returns `null` for `Text` and a text for `Binary` and `TooLarge`.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/diff && (cd src-tauri && cargo test diff::)`
Expected: FAIL

- [ ] **Step 3: Implement the diff**

Use `git diff-tree -r -z -M --root --no-commit-id --name-status`, because `-z` stops Git from escaping paths. For each side, check the size with `git cat-file -s` before reading with `git cat-file blob`. A NUL in the first 8 KB indicates binary.

`DiffView` uses `MergeView` from `@codemirror/merge`, without editing, with `collapseUnchanged` turned on so that long files stay smooth. If a side is not `Text`, it shows `sideMessage`.

- [ ] **Step 4: Check that they pass**

Run: `pnpm vitest run src/diff && (cd src-tauri && cargo test diff::)`
Expected: PASS

- [ ] **Step 5: Check in the app**

Run: `pnpm tauri dev`, and type the path of a repository in the text field.
Expected: when a commit is chosen its file list appears; when a file is chosen the side-by-side diff appears.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: diff view of a commit"
```

### Task 7: Consumption measurement

**Files:**
- Create: `src/perf/run.ts`, `src/perf/stats.ts`, `src/perf/stats.test.ts`, `scripts/measure.py`, `scripts/test_measure.py`, `docs/reports/phase-0-consumption.md`
- Modify: `src/App.svelte`, `src-tauri/src/commands.rs`

**Interfaces:**
- Consumes: `LogView`, `DiffView`, commands from Tasks 5 and 6.
- Produces:
  - Command `perf_report(json: String)`, which writes the JSON to the path in `RAMA_PERF_OUT`
  - `frameStats(deltasMs: number[]) -> { frames, p50, p95, p99, over50ms }`
  - `measure.py --app <binary> --repo <path> --out <json> --runs 5`
  - `measure.py --pid <pid> --out <json>`, to measure IntelliJ

- [ ] **Step 1: Write the failing tests**

- `stats.test.ts`, `computes_percentiles`: of 100 values, `p95` and `over50ms` are the expected ones.
- `test_measure.py`, `test_parse_footprint`: from a text with `phys_footprint: 2304 KB`, `parse_footprint` returns `2359296` bytes; it also accepts `MB`, `GB` and `B`.
- `test_new_processes`: `new_webkit_pids(before, after)` returns only the PIDs that appear afterwards.

- [ ] **Step 2: Check that they fail**

Run: `pnpm vitest run src/perf && python3 -m unittest scripts/test_measure.py`
Expected: FAIL

- [ ] **Step 3: Implement the measurement mode in the app**

With `RAMA_PERF_OUT` defined, the app:
1. Records the time from the start of the process until the first rows are drawn.
2. Opens the most recent commit with changes and its first text file.
3. Scrolls the log from start to end in 10 seconds with `requestAnimationFrame`, saving the duration of each frame.
4. Sends the result with `perf_report` and stays idle.

Without that variable, none of this runs.

- [ ] **Step 4: Implement `measure.py`**

WebKit processes are not children of the app, so they are identified by difference: list the PIDs with `com.apple.WebKit` before and after launching the app. Close other WebKit-based apps during the measurement, or the results get mixed.

For each run:
1. Launch the app in release mode with `RAMA_PERF_OUT`.
2. Wait for the report and 30 seconds of rest.
3. Sum `phys_footprint` with `footprint --noCategories <pid>` over all the processes.
4. Measure CPU with `ps -o %cpu` for 30 seconds without changes to files.
5. Close the app.

Keep the median of 5 runs. If `footprint` cannot read the WebKit processes, the script stops and says it has to be run again with `sudo`.

- [ ] **Step 5: Check that the tests pass**

Run: `pnpm vitest run src/perf && python3 -m unittest scripts/test_measure.py`
Expected: PASS

- [ ] **Step 6: Prepare the measurement repository**

Run: `git clone https://github.com/git/git ~/rama-medida/git && git -C ~/rama-medida/git rev-list --all --count`
Expected: a count close to 100,000. Note it in the report, whatever it is.

Do not use a partial clone: the diff would have to download blobs and the measurement would not be valid.

- [ ] **Step 7: Measure the app**

Run: `pnpm tauri build --no-bundle && python3 scripts/measure.py --app src-tauri/target/release/rama --repo ~/rama-medida/git --out docs/reports/phase-0-consumption.json --runs 5`
Expected: JSON with memory, CPU, startup and frames.

- [ ] **Step 8: Measure IntelliJ as a reference**

Ask the user to open `~/rama-medida/git` in IntelliJ 2023.2.3, with the Git window and a diff open, and to wait for it to finish indexing. Then, with the PID of IntelliJ:

Run: `python3 scripts/measure.py --pid <pid> --out docs/reports/phase-0-intellij.json`
Expected: JSON with memory and CPU.

- [ ] **Step 9: Write the report**

In `docs/reports/phase-0-consumption.md`, a table of each goal with its value, the value for IntelliJ and whether it is met. Scrolling is met if `over50ms` is under 1 % of the frames (threshold of this plan). Also include the WebView `footprint` separately. If something is not met, explain the measured cause, without adjusting the goals.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "feat: consumption measurement and phase 0 report"
```

### Task 8: Parity matrix

**Files:**
- Create: `scripts/extract_actions.py`, `scripts/test_extract_actions.py`, `docs/parity-matrix.md`

**Interfaces:**
- Produces: `extract_actions(xml: str) -> list[Action]`, with `Action(id, cls, group)`, and `docs/parity-matrix.md` with the approved feature list.

- [ ] **Step 1: Write the failing test**

`test_extracts_actions`: from an XML with an `<action id="Git.Init" class="git4idea.actions.GitInit">` inside `<group id="Git.Menu">`, it returns `Action("Git.Init", "git4idea.actions.GitInit", "Git.Menu")`.

- [ ] **Step 2: Check that it fails**

Run: `python3 -m unittest scripts/test_extract_actions.py`
Expected: FAIL

- [ ] **Step 3: Implement the extractor**

Use `xml.etree.ElementTree`. Support nested groups and keep the immediate group.

- [ ] **Step 4: Check that it passes**

Run: `python3 -m unittest scripts/test_extract_actions.py`
Expected: PASS

- [ ] **Step 5: Create the matrix**

Download from the tag `idea/232.10072.27`:
- `plugins/git4idea/resources/META-INF/plugin.xml`, for actions and configuration.
- The default shortcuts of the `Git.*`, `Vcs.*` and `ChangesView.*` actions from the tag's macOS keymaps.

Start from section 5 of the design and add whatever is missing. Each row carries: feature, action id if there is one, shortcut, phase (1 to 4) and status (`include`, `out` or `unsure`). Include the settings of `Settings > Version Control > Git`. Features tied to code analysis go as `out`, citing section 3.

- [ ] **Step 6: Confirm the user's configuration**

Read only these files from `~/Library/Application Support/JetBrains/IdeaIC2023.2/options/`: `ui.lnf.xml`, `laf.xml`, `keymap.xml` and `vcs.xml`, if they exist. Note in the matrix the interface, theme and keymap in use, and the Git settings that are not the factory ones. Do not copy tokens or credentials.

- [ ] **Step 7: Review with the user**

Hand over the matrix together with the report from Task 7. The user resolves each `unsure` and approves the list. This approval is the condition for starting phase 1.

If the report shows that the goals are not reachable, stop here and decide the technology again with the user, as section 6 of the design says.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "docs: parity matrix with IntelliJ 2023.2.3"
```
