# Phase 2 commands: rama versus IntelliJ

Comparison of what the app launches in checkout, new branch, fetch, update, pull, push and force push with what the plan says (`docs/superpowers/plans/2026-11-01-phase-2-log-branches-and-remotes.md`, Tasks 9, 11, 12, 13 and 14) and IntelliJ's code at tag `idea/232.10072.27` (2023.2.3), according to the reference summaries (`ref-branches-push.md`, `ref-update.md`) and the files `GitImpl.java`, `GitUpdateProcess.java`, `GitMergeUpdater.java`, `GitRebaseUpdater.java`, `GitPull.java` and `GitPushOperation.java`.

The IntelliJ column comes from reading that code, not from a running IDE. The live comparison is left for the user (list in `phase-2-validation.md`), with `scripts/git-wrapper.sh` as IntelliJ's Git executable, as in phase 1. The exact text of Update's `git stash save` message is not in the reference: only Checkout's has been checked.

## Scenario

Recorded with the test `phase_2_trace` (in `src-tauri/src/push.rs`, ignored by default), with Homebrew's git 2.38.0, which is the minimum version. A disposable repository with a `main` branch that tracks a local bare remote `origin`, another clone that pushes commits to `main` between one operation and the next, a local branch `side` that changes `a.txt`, and a branch `remote` only on the remote. Each operation calls the core function that the Tauri command uses, inside a `Runner` task, with the same task type as the command (`vcs_commands.rs`). It goes through the core, not through Tauri's IPC or the interface.

The preparation of each operation (commits, branches, the other clone) is not recorded: the log is turned on only while the operation runs.

## How to repeat it

```bash
trace=$(mktemp -d)
mkdir "$trace/bin"
ln -s "$PWD/scripts/git-wrapper.sh" "$trace/bin/git"
(cd src-tauri && RAMA_GIT_REAL=$(command -v git) RAMA_GIT_LOG="$trace/commands.log" \
  PATH="$trace/bin:$PATH" cargo test --lib phase_2_trace -- --ignored)
cat "$trace/commands.log"
```

## Recorded branch commands

Real output. The prefix `-c core.quotepath=false -c log.showSignature=false` is removed, which the commands launched by the task carry (as in IntelliJ); the state reads of `git.rs` do not carry it. Hashes are cut to 7 characters and the stash date is replaced. The reads that change nothing and are not IntelliJ's are omitted (170 of the 267 lines in total): `rev-parse --git-path rebase-apply|rebase-merge|MERGE_HEAD`, `ls-files --unmerged`, `symbolic-ref -q --short HEAD`, `for-each-ref --format=%(upstream…)`, `check-ref-format`, `show-ref --verify -q`, `ls-files -z -v`, `ls-tree -r -z`, `remote`, `rev-parse -q --verify refs/stash`, `status --porcelain=v1` and `diff … --name-only -z -- <path>`.

## Log

### checkout of a local branch, clean

```text
git rev-parse -q --verify refs/heads/side\^\{commit\}
git checkout side --
```

3 state reads are omitted (see below).

### smart checkout with local changes that git does not let be overwritten

```text
git stash save Uncommitted\ changes\ before\ Checkout\ at\ <date>
git rev-parse -q --verify refs/heads/side\^\{commit\}
git checkout side --
git stash list --format=%H
git stash pop stash@\{0\}
```

12 state reads are omitted (see below).

### force checkout

```text
git rev-parse -q --verify refs/heads/side\^\{commit\}
git checkout --force side --
```

9 state reads are omitted (see below).

### checkout of a remote branch

```text
git rev-parse -q --verify origin/remote\^\{commit\}
git checkout -b remote origin/remote --
```

5 state reads are omitted (see below).

### new branch from another, with checkout

```text
git rev-parse -q --verify main\^0\^\{commit\}
git checkout -b new main\^0 --
```

3 state reads are omitted (see below).

### new branch without checkout

```text
git branch loose HEAD
```

1 state read is omitted (see below).

### fetch

```text
git fetch origin --recurse-submodules=no --progress --prune
```

1 state read is omitted (see below).

### update by merge, with overlapping local changes

```text
git fetch origin --recurse-submodules=no --progress --prune
git rev-list -1 HEAD..origin/main
git merge-base main origin/main
git rev-parse HEAD
git diff --name-only --no-renames HEAD
git diff --name-only --no-renames --cached
git diff --name-only --pretty=format: main..origin/main
git stash save Uncommitted\ changes\ before\ Update\ at\ <date>
git merge origin/main --no-stat -v
git stash list --format=%H
git stash pop stash@\{0\}
git merge-base main origin/main
git rev-list --count c998160..4b3169e
git diff --name-only --diff-filter=ADMRUX --no-renames fe81043..HEAD
```

22 state reads are omitted (see below).

### update by rebase, with local changes

```text
git fetch origin --recurse-submodules=no --progress --prune
git rev-list -1 HEAD..origin/main
git merge-base main origin/main
git rev-parse HEAD
git diff --name-only --no-renames HEAD
git merge origin/main --ff-only
git log --merges -c --name-only --format= origin/main..main
git stash save Uncommitted\ changes\ before\ Update\ at\ <date>
git rebase origin/main
git stash list --format=%H
git stash pop stash@\{0\}
git merge-base main origin/main
git rev-list --count 4b3169e..8e5e675
git diff --name-only --diff-filter=ADMRUX --no-renames 5614078..HEAD
```

22 state reads are omitted (see below).

### push of the current branch

```text
git push --progress --porcelain origin refs/heads/main:main
git rev-list --count 8e5e675..b61f9db --
```

7 state reads are omitted (see below).

### pull without options

```text
git merge-base main origin/main
git rev-parse HEAD
git pull --no-stat -v --progress origin main
git merge-base main origin/main
git rev-list --count b61f9db..c34dfe7
git diff --name-only --diff-filter=ADMRUX --no-renames b61f9db..HEAD
```

8 state reads are omitted (see below).

### pull with --rebase (goes through Update)

```text
git fetch origin --recurse-submodules=no --progress --prune
git rev-list -1 HEAD..origin/main
git merge-base main origin/main
git rev-parse HEAD
git diff --name-only --no-renames HEAD
git log --merges -c --name-only --format= origin/main..main
git rebase origin/main
git merge-base main origin/main
git rev-list --count c34dfe7..dadcdd0
git diff --name-only --diff-filter=ADMRUX --no-renames fe81863..HEAD
```

19 state reads are omitted (see below).

### push of a new branch

```text
git push --progress --porcelain origin refs/heads/new:refs/heads/new --set-upstream
```

5 state reads are omitted (see below).

### push rejected after rewriting the history

```text
git push --progress --porcelain origin refs/heads/new:new
git log --merges -c --name-only --format= refs/remotes/origin/new..refs/heads/new
```

11 state reads are omitted (see below).

### force push with lease

```text
git rev-parse --verify -q refs/remotes/origin/new
git push --progress --porcelain origin refs/heads/new:refs/heads/new --force-with-lease=refs/heads/new:ed304a6
```

7 state reads are omitted (see below).

### force push without lease

```text
git push --progress --porcelain origin +refs/heads/new:refs/heads/new
```

7 state reads are omitted (see below).

### retry of a rejected push: Update by merge and push

```text
git fetch origin --recurse-submodules=no --progress --prune
git rev-list -1 HEAD..origin/main
git merge-base main origin/main
git rev-parse HEAD
git diff --name-only --no-renames HEAD
git diff --name-only --no-renames --cached
git diff --name-only --pretty=format: main..origin/main
git merge origin/main --no-stat -v
git merge-base main origin/main
git rev-list --count dadcdd0..ce6d28a
git diff --name-only --diff-filter=ADMRUX --no-renames b197295..HEAD
git push --progress --porcelain origin refs/heads/main:main
git rev-list --count ce6d28a..8b106a9 --
```

28 state reads are omitted (see below).


## IntelliJ commands, according to the code

```text
git checkout <branch> --                                        GitImpl.java:283
git checkout --force <ref> --                                   Force Checkout
git stash save "Uncommitted changes before Checkout at <date>"  GitStashChangesSaver; plan, Task 9
git stash pop [--index]                                         --index only with staging area
git checkout -b <branch> <remote/branch> --                     GitBranchCheckoutOperation.kt:26
git checkout -b <n> <start>^0 --                                GitNewBranchDialog.kt:43
git branch <n> <start>                                          without checkout
git fetch <remote> --recurse-submodules=no --progress --prune   GitImpl.java:612-630
git rev-list -1 HEAD..<remote/branch>                           GitUpdater.java:194-201
git merge <remote/branch> --ff-only                             GitRebaseUpdater.java:93-123, with local changes
git merge <remote/branch> --no-stat -v                          GitMergeUpdater.java:74-76
git rebase <remote/branch>                                      GitRebaser.java
git pull --no-stat <options> -v --progress <remote> <branch>    GitPull.java:131-161
git push --progress --porcelain <remote> <source>:<target> [--set-upstream] [--force-with-lease=<branch>:<sha> | --force] [--tags | --follow-tags] [--no-verify]
                                                                GitImpl.java:505
```

## Differences

| # | Point | IntelliJ | rama | Does it change the result? | Decision |
|---|---|---|---|---|---|
| 1 | Checkout, new branch, fetch, merge, rebase, pull and push | The commands above | The same, with the same arguments and in the same order | No | Matches |
| 2 | `git pull` | `--no-stat -v --progress <remote> <branch>`; with `--rebase` it does not launch `git pull` but Update | The same in both cases (`pull without options` and `pull with --rebase`) | No | Matches |
| 3 | Update | Fetch, `rev-list -1`, `--ff-only` if it is a rebase with local changes, save, `merge`/`rebase`, restore | The same, and in that order. Before saving, with merge, it launches the two reads of `isSaveNeeded` (`diff --name-only --no-renames --cached` and `main..origin/main`); with rebase, `diff --name-only --no-renames HEAD` | No | Matches |
| 4 | Count of the result (`N files updated in M commits`) | `git log <start>..<end>` | `rev-list --count <start>..<end>`; the files, with `diff --name-only --diff-filter=ADMRUX --no-renames <before>..HEAD` and `ls-files --unmerged`, like `MergeChangeCollector` | No | rama is kept: it does not need to read the commits |
| 5 | Restoring what was saved | `git stash pop [--index]`, of the top entry | `git stash list --format=%H` and `git stash pop stash@{n}`, with the index of the entry the app saved | Yes if someone saves a stash by hand in between: IntelliJ would pop the foreign one. Tested in `restoring_does_not_take_another_stash` and `another_hand_made_stash_does_not_confuse_it` | rama is kept |
| 6 | Smart Checkout | First a normal checkout; if git says it would overwrite files, it opens the dialog and then saves | The interface does the same (`problem_offers_smart_and_force_and_retries`). The trace calls Smart mode directly, so it does not show the prior normal checkout | No | Matches. What is recorded is only the second step |
| 7 | Before a checkout | Only the checkout | `rev-parse -q --verify <ref>^{commit}`, `ls-files -z -v` and `ls-tree -r -z <tree>`, to copy the unversioned or ignored files that the checkout would overwrite (review focus 4) | It does not change the repository; it adds three reads | rama is kept |
| 8 | Before saving, in Smart | `git stash save` | Before: rebase and merge state (`rev-parse --git-path`, `ls-files --unmerged`), `status --porcelain`, and `diff --cached` and `diff` of the paths, to copy what is only in the stage (ruling of Task 9) | Yes: IntelliJ loses that stage with staging off | rama is kept |
| 9 | Credential helper | With `Use credential helper` off, which is the default, it adds `-c credential.helper=` | It does not add it: it comes on (decision 3). With the setting off it does (`helper_off_adds_an_empty_config`) | Yes: with a system helper, git uses it | Decision 3 |
| 10 | Force push with lease | `--force-with-lease=<branch>:<sha>` | `--force-with-lease=refs/heads/<branch>:<sha>`, with the whole reference | Yes: the short name matches an equal tag | Ruling of Task 14 |
| 11 | Force push without lease | `--force` | `+refs/heads/<branch>:refs/heads/<branch>` | Yes: with `Push tags: All`, `--force` overwrote remote tags | Ruling of Task 14 |
| 12 | Push of a new branch | `<source>:<target> --set-upstream`, with `refs/heads/` in the target | The same: `refs/heads/new:refs/heads/new --set-upstream` | No | Matches |
| 13 | After a push | Reads the result of `--porcelain` | Also `rev-list --count <remote>..<branch> --` to count what was pushed; after a rejection, `log --merges -c --name-only --format=` to decide what to recommend | It does not change the repository | rama is kept |
| 14 | Before a push | No prior reads that appear in the reference | `check-ref-format` of the branch, `remote`, `show-ref`, `for-each-ref %(upstream…)`, to fix the target and the upstream | It does not change the repository | rama is kept |
| 15 | Rejection and retry | Update (merge or rebase according to the setting) and push again, up to 10 times | The same, with `update_and_push`; it does not retry on force, nor on a branch that is not the current one, nor if saved changes remain (ruling of Task 14) | Yes in the last one: IntelliJ does go on | Ruling of Task 14 |
| 16 | Prefix `-c core.quotepath=false -c log.showSignature=false` | On all | On the task's commands and on `log`; the direct reads of `git.rs` (`rev-parse --git-path`, `symbolic-ref`, `for-each-ref`, `show-ref`, `ls-files --unmerged`, `status`, `remote`) do not carry it | No: they use `-z` or do not read the log | Minor pending item of Task 4 (direct `git::run` without `log.showSignature=false`) |
| 17 | Stash in Update | `git stash save "<message>"`, without `-u` | `git stash save Uncommitted changes before Update at <date>`, without `-u` | To be checked: the text of Update's message is not in the reference | Compare live |

## What the trace does not cover

- Deleting a branch (local and remote), rename, tags and reset: the plan cites their commands (`git branch -d|-D`, `git push --progress --porcelain origin :refs/heads/x`, `git branch -m`, `git tag`, `git reset --soft|--mixed|--hard|--keep`) and the tests of `branches.rs` cover them (`delete_remote_deletes_the_branch_and_its_locals_if_asked`, `lightweight_and_annotated_tag`, `reset_hard_creates_a_copy_of_what_was_modified`…), but they were not run through `git-wrapper.sh`.
- The background `ls-remote` of incoming, `clone`, `init` and the remotes commands (`remote add|rename|set-url|remove`).
- Conflicts: `Abort` (`reset --merge`, `rebase --abort`) and the result with saved changes.
- How any of this behaves with real ssh or https and credentials: the trace uses a local bare remote.
- The interface: nothing went through Tauri's IPC.

## Checks pending for the user

With IntelliJ closed and opened from a terminal that inherits the variables, and `scripts/git-wrapper.sh` as the Git path in its settings, do the same operations and compare the log with the section above:

```bash
export RAMA_GIT_REAL=$(command -v git)
export RAMA_GIT_LOG=~/intellij-commands-phase-2.log
"/Applications/IntelliJ IDEA CE.app/Contents/MacOS/idea"
```

- Rows 4, 5, 8, 13 and 17 of the table.
- The `git stash save` message of Update.
- That `git stash pop` carries `--index` only with staging area on.
