# Phase 2: decisions and pending items

Record of what was decided during execution and of the minor findings that were left unattended. Each decision says what it costs if it turns out to be wrong. The 12 decisions of the plan (Decisions section of `docs/superpowers/plans/2026-11-01-phase-2-log-branches-and-remotes.md`) are noted in section 4 of the design and are not repeated among the execution ones.

The labels come from the execution log, with these changes: a `Ruling` is a line of Decisions (rulings 1 to 3 of the plan's previous review, and those that follow it, are `Preparation`), and `minor (deferred)` is a line of Minor findings. `Final` is the final review of the branch, and its batch of fixes.

## Where the app differs from IntelliJ on purpose

Only what was decided: the plan's decisions and the task rulings. What the validation found without anyone deciding it is in `phase-2-validation.md`.

From the plan's decisions (the user approved them together with it):

- Everything is saved with `git stash`: there is no Shelve until phase 4. `Clean working tree using` is shown with Stash fixed (decisions 1 and 2).
- `Use credential helper` comes enabled, and the login dialog has no `Remember` (decision 3).
- After an error or a cancellation of Update, what was saved is restored if the repository went back to its clean state; IntelliJ leaves it saved (decision 4). If it stays saved, `Restore Changes` does `git stash pop` of that entry, instead of linking to the shelf (decision 5).
- Conflicts are left as git leaves them, with `Abort`; resolving them, `Continue` and `Skip` belong to phase 3 (decision 6).
- Protected branches are a list of patterns per repository, with `master` and `main`; GitHub's rules go to phase 4 (decision 7).
- Filtering asks for all the commits at once, without the stages of 5 and 2,000 (decision 8). The log is reread entirely when the references change, and only if it is visible (decision 9).
- No effect: IntelliJ's index rows, the Roots column and grouping by repository (decision 10). To phase 4: `Vcs.Log.ShowAllAffected`, `Vcs.Log.ShowOtherBranches`, `Select in Tree…` and the comparisons of the log menu (decision 11).
- The log's row height is 24, chosen because IntelliJ computes it from the font (decision 12).

From the task rulings:

- References other than branches and tags (`refs/pull`, `refs/notes`) are not shown in the log (Task 3).
- The `merge_sort` order stays: IntelliJ's comparator is not transitive and IntelliJ is not even deterministic there (Task 4). The `HashSet` order with more than 10 nested merges is not emulated (Task 5). `isForDelegateGraph` and `expandNodes` are not ported (Task 6).
- Smart Checkout and Smart Reset with staging off first copy what is only in the stage; IntelliJ loses it (Task 9). If `pop --index` fails, the `apply` is kept with its own result `RestoredWithoutIndex` and the stash entry; IntelliJ leaves the tree clean (Task 9).
- Deleting a remote branch uses `:refs/heads/x` instead of `:x`, so as not to touch a tag with that name (Task 9). `Rebase onto Remote` makes a prior copy and leaves a warning with `Abort`; saving changes and resolving conflicts is left for phase 3 (Task 9). `Pull into 'cur' Using Merge/Rebase` stays in phase 3 (Task 10).
- Update Project takes the write lock only from saving to restoring, and the fetch goes outside it (Task 11). The stash entries saved by the app are noted by sha and offered again when the repository is opened (Task 11). Quitting with a Write task in progress cancels it and waits up to 60 s (Task 11). There is its own result `RestoreBlocked` with the files (Task 11). Smart Checkout and Smart Reset refuse with a half-finished operation (Task 11).
- Force push anyway forces only the branch, with `+refs/heads/x:refs/heads/x` instead of `--force`, and the lease goes with `refs/heads/<name>` (Task 14). A push is not retried if saved changes remain; IntelliJ does retry it (Task 14).
- The Git menu has no native accelerators (Task 15). It has 13 entries of `Git.MainMenu` (Task 15 and Final).
- `Reset Head` and `Tag` of the Git menu do not have the `Validate` button, and `Reset Head` does not have the root selector either (Final).

## Decisions

- Preparation: T4 defines ViewSpec without a filter field; T6 adds it — the type does not exist yet — cost if wrong: one line.
- Preparation: no_collapse_with_details_filter is written in T6, not in T5 — it needs filters — cost if wrong: none.
- Preparation: T4 adapts perf/run.ts to the new signature of log_rows so that it keeps compiling; T16 moves it to LogPanel — cost if wrong: minimal.
- Preparation: in T10 the Update Project, Commit, Push and Fetch rows of the popup receive injected handlers and come out disabled if there are none; T11, T12 and T13 connect them — cost if wrong: disabled rows between tasks.
- Preparation: T10 adds the incoming_check setting with its row in Settings; T14 adds the others and does not repeat it — cost if wrong: minimal.
- Preparation: commit_and_push_with_amend_warns_of_rejection means that after amending an already pushed commit the rejected push reaches PushRejectedDialog — the name was ambiguous — cost if wrong: one test.
- Task 1: Vcs.ShowConsoleTab stays without a shortcut and keymap.ts untouched — the Mac shortcut is not in the reference — cost if wrong: add one line.
- Task 2: emit_to ignored and the test that ssh dies become Important — the Review Focus demands not hanging and killing ssh — cost if wrong: a somewhat larger round.
- Task 3: references other than branches and tags (refs/pull, refs/notes) are not shown and RefKind has no Other — the log only loads HEAD, branches and tags, and IntelliJ would only paint them if a loaded commit carries them — cost if wrong: one label fewer in rare cases.
- Task 4: click and cursor on arrows, Jump to tooltip and selection by hover of the graph are connected in Task 8 — they are navigation and no task assigned them — cost if wrong: a somewhat larger task.
- Task 4: Task 16 adds the Apache 2.0 attribution of what was ported and of the fixtures — section 8 of the design requires it and the plan had no step for it — cost if wrong: a NOTICE file.
- Task 4: the merge_sort order stays — IntelliJ's comparator is not transitive and sort_by could abort; the reviewer confirmed it with a Java port, and IntelliJ is not even deterministic there — cost if wrong: odd rows with another order.
- Task 5: the HashSet order, which only matters with more than 10 nested merges, is not emulated — ten lines for a very rare case with no effect on the content — cost if wrong: different order in those merges.
- Task 5: the click on an edge and the hover extend log_graph_click in Task 8 — it is already the task for the interaction with the graph — cost if wrong: minimal.
- Task 6: isForDelegateGraph and expandNodes are not ported — the reviewer confirmed they would be dead code — cost if wrong: one click case under a filter.
- Task 6: Select in Tree… moves to phase 4 — it needs the file tree, which belongs to that phase — cost if wrong: one extra step until phase 4.
- Task 6: the submenus are made in Menu.svelte in this task — Tasks 8 and 10 need them — cost if wrong: none.
- Task 6: favorites of the Branch popup and users are connected in Task 10, which adds filters.ts and FilterBar.svelte to its list — cost if wrong: minimal.
- Task 6: a full hash in Select… goes to LogFilter.revisions as a head, not to hashes — confirmed by VcsLogClassicFilterUi.java:490-498 — cost if wrong: none, it was an error of the finding.
- Task 7: the vcs.log.max.changes.shown limit is compared with the largest commit, not with the sum — that is how MainFrame.java:363 does it — cost if wrong: none.
- Task 8: Copy Revision Number joins with a space like the installed app, not with a line break as the brief said — IntelliJ rules — cost if wrong: one line.
- Task 8: Push All up to Here is disabled only with a detached HEAD — the source only requires being on a branch — cost if wrong: minimal.
- Task 9: Smart Checkout and Smart Reset with staging off first copy what is only in the stage — IntelliJ loses it, but section 4 of the design asks for a copy — cost if wrong: one extra copy.
- Task 9: the apply is kept after pop --index fails, with its own result RestoredWithoutIndex and the entry kept — IntelliJ leaves the tree clean and loses sight of the work; here it returns to the disk — cost if wrong: a leftover stash entry.
- Task 9: deleting a remote uses :refs/heads/x instead of :x — :x can delete a tag with that name and fails if both exist; differs from IntelliJ — cost if wrong: another command in the console.
- Task 9: Rebase onto Remote makes a prior copy and leaves a warning with Abort; saving changes and resolving conflicts is left for phase 3 — decision 6 — cost if wrong: a rebase with a dirty tree fails with a git error.
- Task 10: Pull into 'cur' Using Merge/Rebase stays in phase 3 with Merge and Rebase — they are the same family — cost if wrong: one entry fewer until phase 3.
- Task 10: ls-remote carries IntelliJ's authentication gate, only to remotes with correct fetch, pull, push or ls-remote in the session — the plan lost that clause, and contacting remotes without a user action can bring up system dialogs — cost if wrong: incoming commits take longer to show up until the first fetch.
- Task 10: the ls-remote after the popup's fetch is removed — IntelliJ does not do it and the plan said it wrongly — cost if wrong: minimal.
- Task 11: Update Project takes the write lock only from saving changes to restoring; the fetch goes outside — this way a slow fetch does not block commits — cost if wrong: another command between fetch and merge, which is already checked again.
- Task 11: the stash entries saved by the app are noted by sha in one file per repository and offered again on opening — there is no stash interface until phase 4 and the warning lives in memory — cost if wrong: one extra warning.
- Task 11: quitting the app with a Write task in progress cancels it and waits for it to finish, up to 60 s — closing between stash and restore leaves changes off the disk — cost if wrong: closing takes longer.
- Task 11: the assignment Failed is kept to its own result RestoreBlocked with the files — the previous message pushed toward discarding Devin's version — cost if wrong: one text.
- Task 11: Smart Checkout and Smart Reset refuse with a half-finished operation — stash save deletes MERGE_HEAD and pop leaves the resolution out of the stage; IntelliJ does not protect it the same way — cost if wrong: a refusal with a clear message.
- Task 12: pull still goes without --no-rebase and with divergent branches and without pull.rebase gives git's error — that is how IntelliJ 2023.2 behaves with git 2.33 or later — cost if wrong: the user will see a git error and uses Update Project.
- Task 13: PENDING_OPERATIONS is removed in Task 14 if it is still empty — unused mechanism — cost if wrong: minimal.
- Task 14: Force push anyway forces only the branch, with +refs/heads/x:refs/heads/x instead of --force — with Push tags: All, --force overwrote remote tags without warning; differs from IntelliJ — cost if wrong: another form of the command in the console.
- Task 14: the lease goes with refs/heads/<name> — the short name matches an equal tag — cost if wrong: minimal.
- Task 14: do not retry a push if saved changes remain — the user's work comes first; IntelliJ does go on — cost if wrong: one more manual push.
- Task 15: the Git menu has no native accelerators — the keyboard in JS already handles them and who wins cannot be tested with the screen locked — cost if wrong: the menu does not show the shortcuts.
- Task 15: the Git menu has 11 entries as Git.MainMenu — without a Pull menu it had no entry — cost if wrong: none.
- Final: Important 1 and 2, shortcuts, token as user, license and documentation are handled in a single batch — the shortcuts change habits and are cheap — cost if wrong: a somewhat larger batch.
- Final: 3.1 % CPU with the diff window stays unfixed and as a manual experiment — it belongs to phase 1 and the cause has not been measured — cost if wrong: idle CPU above the target.
- Final: the Git menu goes to 13 entries, with New Tag… and Reset HEAD… — the text of Git.Tag is New Tag…, not Tag… (GitBundle.properties:557) — cost if wrong: an entry with another name.
- Final: F2, ⇧F6, ⌥F1 and ⌘⌥N are bound only in the branches panel — those actions read data that only BranchesTreePanel gives (BranchesDashboardUi.kt:281-290), and the popup gives other data (GitBranchesTreePopupStep.kt:240-246) — cost if wrong: one shortcut fewer in the popup.
- Final: Reset HEAD… does not remember the mode and does not warn if it goes well, and Reset Head and Tag stay without Validate — it is GitResetHead.java; Validate would ask for a new command — cost if wrong: a badly written reference ends in git's error.
- Final: a URL user without `:` and with 20 characters or more, without `@`, is hidden as a token — usual tokens reach that length, and a name rarely does — cost if wrong: a long name comes out as ***.

## Minor findings not attended

- Task 1: pipes joined after wait: a descendant that leaves the group leaves the task uncancelled
- Task 1: no catch_unwind in start, in debug a task that panics becomes a ghost
- Task 1: hide_credentials does not hide a token used as user in https://ghp_x@host (applied in the final batch)
- Task 1: no cancelling of tasks on quitting the app, git stays alive
- Task 1: mem::forget of TempDir in the test Harness
- Task 1: map of locks per remote does not shrink
- Task 1: ConsoleView copies the array per line, in bursts it costs O(n)
- Task 1: progress() discards any `Text: NN%` line of stderr
- Task 1: obsolete comment of start about the result
- Task 1: taskResult waits forever if the id left the 200 kept; trackTasks does not clean finished
- Task 2: isSecondPrompt sends the password to any holder of the token with the same URL and user for 5 s
- Task 2: cancelling SSH Password Login with real ssh may retry up to 3 times; check with the first real fetch
- Task 2: tests/askpass.rs does not isolate GIT_CONFIG_GLOBAL and uses a global env::set_var
- Task 2: unrecognized question goes whole into the dialog title, without line breaks
- Task 2: Server::drop does not empty pending
- Task 2: resume_and_notify emits repo-changed on every question
- Task 2: start_askpass without an automatic test: there is no simulated Tauri runtime
- Task 3: global Refs.tracked: differs from IntelliJ if the configured upstream is on another commit, and the order of remotes is not config's
- Task 3: Refs::index_of depends on pointer identity with all
- Task 3: remote_name fails with remotes with a slash
- Task 3: optional_output and optional_line almost duplicated
- Task 3: branch of an empty repository in load hard to read
- Task 3: remove refs_text and its test when the new LogRow arrives
- Task 3: log without -c log.showSignature=false
- Task 4: non_transitive_order_does_not_break does not check determinism or continuity of edges
- Task 4: comment of merge_sort does not state the consequence
- Task 4: JBColor.BLACK is light in a dark theme: review MARK_COLOR and color 0 when connecting the hover in Task 8
- Task 4: direct git::run without log.showSignature=false in commands.rs:1424
- Task 5: collapsing_in_standard_uses_the_rows_of_that_order checks little
- Task 5: log_view_set and log_graph_click retain state.log; in repositories 10 times larger the scroll would wait; use spawn_blocking
- Task 5: log_graph_click discards if there was a change
- Task 5: duplicated dotted, circular dependency collapse and linear_bek, two different MAX_BLOCK_SIZE
- Task 5: View::new computes twice with collapsed
- Task 5: merge_branches silently returns git's order if it does not advance
- Task 5: provenance of testdata/linear_bek without a header
- Task 6: texts: tooltip Text or Hash Filter, full tooltip, not cut at 40, pathsText at 30 with trailing slash
- Task 6: me hidden without a git user; an email alone does not count as a user
- Task 6: filter.rs mixes args, selection, edges, users and highlights
- Task 6: hashes_of treats the text as a hash even if Regex is on
- Task 6: Search History from memory: limit 5 and name without source; history per repository
- Task 6: fixed submenus and popup resize not seen in WebKit
- Task 7: new tab without a bar until it opens the view; if it fails, without tabs
- Task 7: LogView without a guard on being destroyed: a late publish may overwrite selection
- Task 7: 1 px divider without a drag zone and without pointercancel
- Task 7: ChangesPanel: Enter fires twice and ↑↓ does not move the file selection
- Task 7: filtered merge prioritizes another message than IntelliJ
- Task 7: ⌘R runs after the Settings overlay
- Task 7: close_view does not cancel pending; next_view restarts on every log_open
- Task 7: DiffView: error does not clear the previous view; text "to the first parent" inexact
- Task 7: LogPanel.svelte 693 lines: extract tabs
- Task 7: tests without their own RED; do mutations
- Task 7: with the preview open, every reload goes back to the first file
- Task 7: FORMAT comment misplaced in log/details.rs
- Task 7: DiffView says "to the first parent" with a different base
- Task 7: perf's openCommit does not resolve if it does not find the hash
- Task 7: rename and delete in multiselection shows state D with oldPath
- Task 8: showArrowTip without catch and RowCache.get without refreshing LRU
- Task 8: hover_collapses_nothing checks nothing
- Task 8: copy_with: zombie if write_all fails, no timeout, no size cap
- Task 8: numbered popup does not look at modifiers
- Task 8: tip and highlight are not cleared on scrolling without moving the mouse
- Task 8: queued click after a sort uses old rows
- Task 8: right-button selection from memory and position of Copy Commit Subject unverified
- Task 8: askHover without try/finally
- Task 9: git branch -d of delete_remote without RepoLocks: narrow race with checkout
- Task 9: commits dropped by -B or -D without the app's durable record, same as IntelliJ
- Task 9: minimal windows when identifying the stash by sha
- Task 9: await names() outside attempt in actions.ts (cause fixed in the validation: the names no longer depend on the log)
- Task 9: rejection because of unversioned files falls to a generic error and leaves a copy on each attempt
- Task 9: reset mode in global localStorage
- Task 9: rebase_abort copies against ORIG_HEAD instead of rebase-merge/orig-head and does not check that there is a rebase
- Task 9: rebase_onto does not remove duplicate paths
- Task 10: Update and Push… icons with incoming and outgoing
- Task 10: New Branch disabled with HEAD selected and Delete of only one branch in the panel
- Task 10: panel builds up to 30000 rows without virtualizing
- Task 10: silent task does not end on quitting: hung ssh outlives the app
- Task 10: env::var_os versus EnvironmentUtil for SSH_AUTH_SOCK
- Task 10: %(push) may mark a false outgoing with push.default=current
- Task 10: speedSearch approximates MinusculeMatcherImpl
- Task 10: hung remote loses what is known about it in that round
- Task 10: protected branch in Rust with a fixed list until Task 14
- Task 11: half a tree after killing a fast-forward or clean merge, without losing the user's work (fixed in Update and in Pull in the validation)
- Task 11: residual race: Devin writes between the stash snapshot and its reset --hard; known limit
- Task 11: update_branch without RepoLocks: race with checkout of that branch
- Task 11: no test of UNMERGED_FILES alone; wiring tests without RED
- Task 11: the Cannot complete update card without a saved entry does not return on reopening; until phase 3 Abort stays without an entry
- Task 11: chooseUpstream and updateProject do not capture generation while the dialog is open
- Task 11: reopening the same repository raises the generation and an Update in progress ends without buttons
- Task 12: stopped_midway gives true if ls-files fails
- Task 12: --no-commit leaves MERGE_HEAD without Abort or warning
- Task 12: pull.rebase=interactive without a rebase editor: git waits for an editor without a terminal; cancelled by hand
- Task 12: test of git_pull without checking the real task_kind or cancellation in merge
- Task 12: order of options by enum, not by selection
- Task 12: update_paths may differ between notification and tab
- Task 12: viewCommits without catch
- Task 12: update::abort uses reset --merge with any state without MERGE_HEAD and says Pull abort succeeded
- Task 12: comment of task_kind misplaced in pull.rs
- Task 13: changing remote does not apply its default destination
- Task 13: completion only with branches of the chosen remote
- Task 13: anchor is not reset on reload
- Task 13: tooltip with subject, not full message
- Task 13: title Cannot load remote branches borrowed from Pull
- Task 13: no test of hide_credentials in the error without porcelain
- Task 13: known_upstream assumes the refs/remotes/<r>/ mapping
- Task 14: on a case-insensitive remote, MAIN skips the protection; same as IntelliJ, document
- Task 14: damaged repository file goes back to the default patterns; the check should deny
- Task 14: protectedTarget uses JS regex; a Rust-only pattern skips the dialog
- Task 14: commit_and_push_with_amend_warns_of_rejection does not amend
- Task 14: push discarded because of switching repository does not warn
- Task 14: with a rejected tag the notification says Force pushed without mentioning it
- Task 14: a RestoredWithoutIndex warning may be covered by a later Update with saved changes
- Task 15: open_in_new_window does not check that it is a repository; /usr/bin/open by absolute path is applied (remotes.rs)
- Task 15: URL with a visible password in RemotesDialog and on removing a remote
- Task 15: bare().keep() leaves temporaries; url_ext depends on the global config
- Task 15: menu enum in commands.rs, already very large
- Task 15: init in a subfolder of the open repository without locks
- Task 15: create_parent runs before validating the URL
- Final: a normal pull holds the lock during the network; a commit waits
- Final: ⌘Q with an unsaved diff and a Write task may be rejected at 10 s
- Final: Dock Quit does not go through finish_writes; to check by hand
- Final: two processes on the same repository compete for pending-restores.json
- Final: when switching repository Restore is withdrawn: what comes from a forced deletion stays without a button in the app, as after closing the notification
