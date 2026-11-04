# Phase 3 commands: rama versus IntelliJ

Comparison of what the app launches in merge (with and without conflict), Smart Merge, rebase (with continue, skip and abort), interactive rebase, cherry-pick, revert, Cherry-Pick and Revert Selected Changes, undo commit, reword, drop and squash, and conflict resolution (Accept Yours, Accept Theirs and Apply), with what the plan says (`docs/superpowers/plans/2026-11-02-phase-3-conflicts-and-rewriting.md`, Tasks 2, 3, 6, 8, 9, 10, 11, 12, 13 and 14) and IntelliJ's code at tag `idea/232.10072.27` (2023.2.3): `GitImpl.java`, `GitFileUtils.java`, `GitMergeUtil.java`, `GitMergeProvider.java`, `GitStashChangesSaver.java`, `GitRewordOperation.kt`, `GitUncommitAction.java`, `GitRevertOperation.kt`, `GitCherryPicker.java` and `GitCommitEditingOperation.kt`.

The IntelliJ column comes from reading that code, not from a running IDE. The live comparison is left for the user (list in `phase-3-validation.md`), with `scripts/git-wrapper.sh` as IntelliJ's Git executable, as in phases 1 and 2.

## Scenario

Recorded with the test `phase_3_trace` (in `src-tauri/tests/phase_3_trace.rs`, ignored by default), with git 2.54.0 (Apple Git-157). Each scenario uses a new throwaway repository and its own data folder. Each operation calls the core function that the Tauri command uses, inside a `Runner` task of type `Write`, with a real editor server: the client is the app's binary, as git launches it. It goes through the core, not through Tauri's IPC or the interface. Where a real command would ask the window for a message (Continue Rebase), the test accepts it as is.

The preparation of each scenario (commits, branches, the earlier conflict) is not recorded: the log is turned on only while the operation runs. In the scenarios that continue a sequence (merge with conflict and its Apply, a rebase and its Continue), the previous step is in its own section.

## How to repeat it

```bash
trace=$(mktemp -d)
mkdir "$trace/bin"
ln -s "$PWD/scripts/git-wrapper.sh" "$trace/bin/git"
(cd src-tauri && RAMA_GIT_REAL=$(command -v git) RAMA_GIT_LOG="$trace/commands.log" \
  PATH="$trace/bin:$PATH" cargo test --test phase_3_trace -- --ignored)
cat "$trace/commands.log"
```

## Recorded commands

Real output. The prefix `-c core.quotepath=false -c log.showSignature=false`, which the commands launched by the task carry (as in IntelliJ), is removed; the state reads of `git.rs` do not carry it. Hashes are cut to 7 characters, the stash date and the temporary paths are replaced. `core.commentChar=\001` is IntelliJ's comment character (`REBASE_CONFIG`). The reads that change nothing and are not IntelliJ's are omitted (347 of the 394 lines in total): `rev-parse -q --verify`, `--git-path` and `--absolute-git-dir`, `ls-files` (`--unmerged`, `-z -v`, `-z -s`), `ls-tree -r -z`, `status --porcelain`, `symbolic-ref`, `for-each-ref`, `check-ref-format`, `cat-file -p :N:<path>`, `diff --cached|… --name-only -z -- <path>`, `merge-base --is-ancestor|--octopus`, `rev-list --no-walk|--topo-order|--parents -n 1`, `log --topo-order|--no-walk|-1 --format=%B`, `diff-tree` and `diff --cached --quiet`.

## Log

### merge without conflict

```text
git merge-base HEAD side
git merge side
git merge-base HEAD side
git rev-list --count 69f08d1..3349730
git diff --name-only --diff-filter=ADMRUX --no-renames 1ededb0..HEAD
```

7 state reads are omitted (see above).

### merge with conflict

```text
git merge-base HEAD side
git merge side
```

6 state reads are omitted (see above).

### merge window: opening the three versions

```text
git merge-base cb9f76a 1148f45
```

15 state reads are omitted (see above).

### Apply (result of the merge window)

```text
git --literal-pathspecs -c core.precomposeunicode=false add -f -- a.txt
```

4 state reads are omitted (see above).

### committing the merge after resolving

```text
git -c i18n.commitEncoding=UTF-8 commit -F <.git>/MERGE_MSG --
```

4 state reads are omitted (see above).

### Accept Yours in a merge

```text
git --literal-pathspecs -c core.precomposeunicode=false checkout --ours -- a.txt
git --literal-pathspecs -c core.precomposeunicode=false add --ignore-errors -A -f -- a.txt
```

5 state reads are omitted (see above).

### committing the merge after Accept Yours

```text
git -c i18n.commitEncoding=UTF-8 commit -F <.git>/MERGE_MSG --
```

4 state reads are omitted (see above).

### Accept Theirs in a merge

```text
git --literal-pathspecs -c core.precomposeunicode=false checkout --theirs -- a.txt
git --literal-pathspecs -c core.precomposeunicode=false add --ignore-errors -A -f -- a.txt
```

5 state reads are omitted (see above).

### Abort Merge

```text
git merge --abort
```

10 state reads are omitted (see above).

### Smart Merge with overlapping local changes

```text
git stash save Uncommitted\ changes\ before\ Merge\ at\ <date>
git merge-base HEAD side
git merge side
git merge-base HEAD side
git rev-list --count bf89771..bba870e
git diff --name-only --diff-filter=ADMRUX --no-renames e9ea410..HEAD
git stash list --format=%H
git stash pop stash@\{0\}
```

23 state reads are omitted (see above).

### rebase without conflict (Rebase…)

```text
git -c core.commentChar=\001 rebase main
```

17 state reads are omitted (see above).

### rebase with conflict

```text
git -c core.commentChar=\001 rebase main side
```

16 state reads are omitted (see above).

### Continue Rebase after resolving

```text
git -c core.commentChar=\001 rebase --continue
```

7 state reads are omitted (see above).

### Skip Commit

```text
git -c core.commentChar=\001 rebase --skip
```

7 state reads are omitted (see above).

### Abort Rebase

```text
git rebase --abort
```

14 state reads are omitted (see above).

### interactive rebase (squash, reword and drop from the dialog)

```text
git -c core.commentChar=\001 rebase --interactive c276591
```

30 state reads are omitted (see above).

### cherry-pick without conflict

```text
git cherry-pick b4c7736
```

4 state reads are omitted (see above).

### cherry-pick with conflict

```text
git cherry-pick 5d28b32
```

4 state reads are omitted (see above).

### committing the cherry-pick after resolving

```text
git -c i18n.commitEncoding=UTF-8 commit -m choca
```

6 state reads are omitted (see above).

### revert with conflict

```text
git revert --no-edit 5f01980
```

4 state reads are omitted (see above).

### Abort Revert

```text
git revert --abort
```

10 state reads are omitted (see above).

### revert without conflict

```text
git revert --no-edit c59c454
```

4 state reads are omitted (see above).

### Cherry-Pick Selected Changes

```text
git --literal-pathspecs diff --binary --full-index --no-renames --no-ext-diff --no-textconv --src-prefix=a/ --dst-prefix=b/ e2a6fa4 b28e22f -- a.txt
git apply --whitespace=nowarn
```

3 state reads are omitted (see above).

### Revert Selected Changes

```text
git --literal-pathspecs diff --binary --full-index --no-renames --no-ext-diff --no-textconv --src-prefix=a/ --dst-prefix=b/ e2a6fa4 b28e22f -- a.txt
git apply --whitespace=nowarn -R
```

3 state reads are omitted (see above).

### undo commit

```text
git reset --soft 1e0bb30 --
```

11 state reads are omitted (see above).

### reword of the last commit (amend)

```text
git commit --amend -F <.git>/rama-reword-Dt4Idb --only --no-verify
```

11 state reads are omitted (see above).

### reword of an earlier commit (rebase)

```text
git -c core.commentChar=\001 rebase --interactive --no-autosquash 74d7eb7
```

25 state reads are omitted (see above).

### drop

```text
git -c core.commentChar=\001 rebase --interactive --no-autosquash 9869086
```

26 state reads are omitted (see above).

### squash

```text
git -c core.commentChar=\001 rebase --interactive --no-autosquash 3f2ea8a
```

29 state reads are omitted (see above).

### interactive rebase with edit (it stops)

```text
git -c core.commentChar=\001 rebase --interactive a255bb9
```

26 state reads are omitted (see above).

### Continue Rebase after edit

```text
git -c core.commentChar=\001 rebase --continue
```

7 state reads are omitted (see above).


## IntelliJ commands, according to the code

```text
git merge <branch> [options]                                    GitImpl.java:262-274
git commit -F <MERGE_MSG> --                                    GitMerger.mergeCommit; plan, Task 7
git merge --abort | cherry-pick --abort | revert --abort        GitAbortOperationAction
git checkout --ours|--theirs -- <path>                          GitMergeUtil.java:355-356
git add --ignore-errors -A [-f] -- <path>                       GitFileUtils.java:188-204 (addPathsImpl)
git stash save "Uncommitted changes before Merge at <date>"     GitStashChangesSaver.java:100; plan, decision 1
git stash pop [--index]
git -c core.commentChar=\001 rebase [options] <base> [<branch>] GitImpl.java:712 (REBASE_CONFIG_PARAMS)
git -c core.commentChar=\001 rebase --continue | --skip         GitImpl.java:753 (rebaseResume)
git rebase --abort                                              GitImpl.java:728-733, without REBASE_CONFIG_PARAMS
git -c core.commentChar=\001 rebase --interactive <parent>      startInteractiveRebase, without --no-autosquash
git -c core.commentChar=\001 rebase --interactive --no-autosquash <parent>
                                                                GitCommitEditingOperation.kt:28 (AutoSquashOption.DISABLE)
git cherry-pick [-x] [-n] <hash>                                GitImpl.java:565-583
git revert <hash> [--no-commit]                                 GitImpl.java:784-796
git commit --amend -F <file> --only --no-verify                 GitRewordOperation.kt:57-72
git reset --soft <parent>                                       GitUncommitAction.java:96-103 (GitResetOperation, SOFT)
git apply [-R] [--3way]                                         plan, decision 8 (IntelliJ uses its PatchApplier)
```

## Differences

| # | Point | IntelliJ | rama | Does the result change? | Decision |
|---|---|---|---|---|---|
| 1 | Merge without options | `git merge <branch>` | `git merge side` | No | Matches |
| 2 | Merge with options (`--no-ff`, `--squash`, `-m`…) | The branch first and the options after it | The options in the order of the dialog (with `-m` and its text right after it) and the branch last. The trace only records the merge without options; the order is fixed by the tests of `merge.rs` (`merge_with_each_option_launches_the_intellij_command`) | No: git accepts both forms | rama is kept (Task 8) |
| 3 | Before and after a merge | No earlier reads that appear in the reference | `ls-files --unmerged`, `rev-parse --git-path rebase-*|MERGE_HEAD`, `merge-base`, and afterwards the count (`rev-list --count <start>..<end>`, `diff --name-only --diff-filter=ADMRUX --no-renames <before>..HEAD`), like row 4 of phase 2 | It does not change the repository | rama is kept |
| 4 | Committing the merge after resolving | `git commit -F MERGE_MSG --` with the repository's encoding | `git -c i18n.commitEncoding=UTF-8 commit -F MERGE_MSG --` | Yes with a repository that is not UTF-8: the message is decoded and written as UTF-8 | Minor pending item of Task 7 |
| 5 | Accept Yours / Accept Theirs | `checkout --ours|--theirs -- <path>` and `add --ignore-errors -A -f -- <path>` (or `rm`) | The same two, with `--literal-pathspecs -c core.precomposeunicode=false` in front. In a rebase the side is inverted (`in_rebase_sides_and_columns_are_swapped`); the trace only covers the merge | No | Matches. The earlier copy of the file (decision 5) is not a git command |
| 6 | Apply of the merge window | `add --ignore-errors -A -f -- <path>` (`conflictResolvedForFile`) | `add -f -- <path>`, after writing the file with an atomic rename | No: the file always exists, and `-A` only changes something when the file is no longer there | rama is kept |
| 7 | Smart Merge | `stash save "Uncommitted changes before Merge at <date>"`, `merge`, `stash pop` (by default, Shelve) | The same, with Stash fixed (decision 1). The `pop` is `stash pop stash@{n}` of the entry that the app saved, after `stash list --format=%H` (row 5 of phase 2). Before it, a copy of what is only in the stage (row 8 of phase 2) | Yes if someone saves a stash by hand in between | Decision 1; rama is kept |
| 8 | Rebase (Rebase… dialog, without conflict) | `git -c core.commentChar=\001 rebase [flags] <base> [<branch>]` | The same: `rebase main` and, with a branch, `rebase main side`. Before: `status --porcelain --untracked-files=no` to decide whether to save, and `ls-files -z -v` and `ls-tree -r -z` to copy the ignored files that the rebase would overwrite | It does not change the repository | rama is kept. The options go in a fixed order, not in the chosen one (minor pending item of Task 8) |
| 9 | Continue, Skip and Abort | `rebase --continue` and `--skip` with `core.commentChar=\001` and with the editor; `rebase --abort` with neither | The same in all three. Before `--abort` it copies the file against `rebase-merge/orig-head` (reads of `diff --cached`, `ls-files -s`) | No | Matches |
| 10 | Interactive rebase from the log | `rebase --interactive <full parent>`, without `--no-autosquash` (`AutoSquashOption.DEFAULT`) | The same. The list is answered by the dialog's plan (`using log` mode, decision 10) and the reword messages arrive through the message editor | No | Matches. Before: `log --topo-order … HEAD ^<hash>^@` to generate the entries, which IntelliJ reads with its log |
| 11 | Reword, Drop and Squash from the log | Reword of the last one with `commit --amend -F <file> --only --no-verify`; the rest with `rebase --interactive --no-autosquash <parent>` | The same in both forms. The message file lives in `.git/rama-reword-*` (IntelliJ: a temporary system file) | No | Matches |
| 12 | Undo commit | `reset --soft <parent>` through `GitResetOperation` | `reset --soft <parent> --`, with the `--` added and without `--hard` or `--mixed` | No | Matches |
| 13 | Cherry-pick | `cherry-pick [-x] [-n] <hash>`; `-x` if the commit is published and the `cherry picked from` setting is on | `cherry-pick <hash>`, without `-n`, and with `-x` in the same case (`apply_one`). The trace only records the case without `-x`. After resolving conflicts it commits with `commit -m <proposed message>`, which adds the suffix to the text by hand | Not in the normal case | rama is kept (Task 10) |
| 14 | Revert | `revert <hash> [--no-commit]` | `revert --no-edit <hash>` | No: `--no-edit` keeps git from opening an editor | rama is kept |
| 15 | Cherry-Pick and Revert Selected Changes | A patch of its own applied with `PatchApplier` | `git diff --binary --full-index --no-renames … <base> <hash> -- <paths>` and `git apply [-R]`; if it does not go in, it is repeated with `--3way` (decision 8). The trace only covers the first attempt | Yes in how a patch that goes in halfway is resolved | Decision 8 |
| 16 | Conflict during `cherry-pick` | git leaves `CHERRY_PICK_HEAD`; IntelliJ removes it to commit with `commit --only` | It keeps it and commits with `commit -m <message>`, after checking with `diff --cached --quiet` | Yes: git keeps the author and the date of the original commit | Ruling of Task 10 |
| 17 | Prefix `-c core.quotepath=false -c log.showSignature=false` | In all | In the task's commands; the reads of `git.rs` (`rev-parse`, `symbolic-ref`, `ls-files`, `status`…) and several of phase 3 (`log --no-walk`, `diff-tree`, `for-each-ref`) do not carry it | No: they use `-z`, a fixed format or do not read the log. `picked_of` does read a `git show` with signatures (minor pending item of Task 3) | Minor pending item |

## What the trace does not cover

- **Rebase with `--onto`, `--root`, `--rebase-merges` and `--keep-empty`**, the branches popup rebase (`rebase_onto`, `branch_rebase`, `rebase_remote`) and Pull into… with merge or rebase: they are covered by the tests of `rebase.rs`, `merge.rs` and `pull.rs`, but they were not run through `git-wrapper.sh`.
- **Rebase in native mode** (a range with a merge, a root or a `fixup!`/`squash!` subject): git's list goes to the window, which does not exist here. And **Fixup… / Squash Into…** (`rewrite::autosquash`), which launches `rebase --interactive <hash>~` with the window's editor.
- **Accept Yours and Accept Theirs during a rebase**, with the sides inverted, and the merge window with deletion conflicts: only the tests of `conflicts.rs`.
- **Restoring what was saved after a rebase or a Smart Merge with conflicts**, and the earlier copy of the overwritten files.
- **Merge with `--squash` and `--no-commit`**, and the commit with `SQUASH_MSG`.
- **The second attempt of Cherry-Pick/Revert Selected Changes with `--3way`**, and its cleaning of the stage.
- **Interactive rebase with a `reword` pending a message when the app is reopened** and the `exec`/`break` mode: the free text editor.
- Hooks (`pre-commit`, `pre-rebase`, `commit-msg`) and real GPG in the reword and the squash.
- The interface: nothing went through Tauri's IPC or through the window.

## Checks pending for the user

With IntelliJ closed and opened from a terminal that inherits the variables, and `scripts/git-wrapper.sh` as the Git path in its settings, do the same operations and compare the log with the section above:

```bash
export RAMA_GIT_REAL=$(command -v git)
export RAMA_GIT_LOG=~/intellij-commands-phase-3.log
"/Applications/IntelliJ IDEA CE.app/Contents/MacOS/idea"
```

- Rows 2, 4, 6, 13, 14 and 16 of the table.
- That the interactive rebase from the log and Edit Commit Message… launch the same as `phase-3-commands.md`, and which comment character arrives in each case.
- What IntelliJ launches when committing a merge with resolved conflicts: `commit -F MERGE_MSG --` or its commit dialog with `--only`.
- Which command Cherry-Pick Selected Changes uses when the patch does not go in cleanly.
