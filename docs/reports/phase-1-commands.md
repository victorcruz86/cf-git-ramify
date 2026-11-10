# Commands of a commit: Ramify versus IntelliJ

Comparison of the flow of `src-tauri/src/commit.rs` with `commitRepository` and `commitUsingIndex` of `GitCheckinEnvironment.java`, `GitRepositoryCommitter.kt` and `GitIndexUtil.java`, in the tag `idea/232.10072.27` (2023.2.3).

The IntelliJ column comes from reading that code, not from a running IDE. The live comparison is left for Task 12, with `scripts/git-wrapper.sh` as IntelliJ's Git executable. `GitChangeUtils.java` and `GitFileUtils.java` are not among the reference files: what depends on them is marked "(unread)", and it has to be confirmed in that comparison.

## Scenario

Partial commit with an excluded stage, in a throwaway repository created by the test `trace_of_partial_commit_with_excluded_stage`:

- `parcial.txt`: selected with partial content.
- `completo.txt`: new and selected whole.
- `borrado.txt`: deleted on disk and selected.
- `excluido.txt`: in stage, with other content on disk, outside the selection.
- `borrado-excluido.txt`: deleted in stage, outside the selection.

## How to repeat it

```bash
trace=$(mktemp -d)
mkdir "$trace/bin"
ln -s "$PWD/scripts/git-wrapper.sh" "$trace/bin/git"
(cd src-tauri && RAMIFY_GIT_REAL=$(command -v git) RAMIFY_GIT_LOG="$trace/commands.log" \
  PATH="$trace/bin:$PATH" cargo test traza_de_commit -- --ignored)
cat "$trace/commands.log"
```

## Commands recorded for Ramify

Real output with git 2.38.0, without the `%q` escapes and with the temporary folder abbreviated. The prefix of the index commands is explained in points 12 and 14; `--literal-pathspecs` makes a path like `:(exclude)a.txt` literal.

```text
git diff --name-only --diff-filter=U -z
git rev-parse -q --verify HEAD
git diff --cached --name-status -z --no-renames
git --literal-pathspecs -c core.precomposeunicode=false ls-files -s -z -- borrado.txt completo.txt parcial.txt borrado-excluido.txt excluido.txt
git --literal-pathspecs -c core.precomposeunicode=false diff --name-only --no-renames --diff-filter=A -z -- borrado.txt completo.txt parcial.txt borrado-excluido.txt excluido.txt
git --literal-pathspecs -c core.precomposeunicode=false ls-tree -r -z HEAD -- borrado-excluido.txt excluido.txt
git --literal-pathspecs -c core.precomposeunicode=false diff --cached --no-renames --no-ext-diff --name-only -z -- parcial.txt
git --literal-pathspecs -c core.precomposeunicode=false diff --no-renames --no-ext-diff --name-only -z -- parcial.txt
git hash-object -w --stdin --path parcial.txt
git -c core.precomposeunicode=false update-index --add --cacheinfo 100644,01e79c32a8c99c557f0757da7cb6d65b3414466d,parcial.txt
git --literal-pathspecs -c core.precomposeunicode=false reset HEAD -- borrado-excluido.txt excluido.txt
git --literal-pathspecs -c core.precomposeunicode=false rm --cached --quiet -f --ignore-unmatch -- borrado.txt
git --literal-pathspecs add --ignore-errors -f -- completo.txt
git -c i18n.commitEncoding=UTF-8 commit -F /var/folders/.../T/ramify-commit-jWvwMG --signoff --
git --literal-pathspecs -c core.precomposeunicode=false update-index --force-remove -- borrado-excluido.txt
git -c core.precomposeunicode=false update-index --add --cacheinfo 100644,e4913bea7055a208a5645d07819e6827873480ee,excluido.txt
git rev-parse HEAD
```

## IntelliJ commands, according to the code

```text
git ls-files -s -- parcial.txt                                       addPartialChangesToIndex → getStagedFile
git hash-object -w --stdin --path parcial.txt                        GitIndexUtil.write
git update-index --cacheinfo 100644,<hash>,parcial.txt --            GitIndexUtil.updateIndex
git <list of unmerged files>                                         GitChangeUtils.getUnmergedFiles (unread)
git diff --cached <name status>                                      GitChangeUtils.getStagedChanges (unread)
git diff <name status> -- <excluded additions>                       GitChangeUtils.getUnstagedChanges (unread)
git reset -- excluido.txt borrado-excluido.txt                       resetExcluded
git rm --ignore-unmatch --cached -r -- borrado.txt                   updateIndex → GitFileUtils.deletePaths
git add <forced> -- completo.txt                                     updateIndex → GitFileUtils.addPathsForce (unread)
git commit -F <file> --signoff --                                    GitRepositoryCommitter.commitStaged
git rm --ignore-unmatch --cached -r -- borrado-excluido.txt          restoreExcluded → updateIndex
git add <forced> -- excluido.txt                                     restoreExcluded → updateIndex
```

## Differences

| # | Point | IntelliJ | Ramify | Does the result change? | Decision |
|---|---|---|---|---|---|
| 1 | Restoring what was excluded | `git add`: the content **on disk** goes back to the stage | `update-index --cacheinfo` with the saved blob | Yes. With `excluido.txt` different in stage and on disk, IntelliJ loses the stage version | Ramify is kept: it is the task's requirement |
| 2 | If the commit fails | Restores what was excluded; what was selected stays in stage | Also restores what was selected, with the previous blob | Yes, only on failures | Ramify is kept: the stage stays as it was |
| 3 | Excluded with status `AD` | Detects it with the unstaged diff and does not restore it | That diff is not needed: `--cacheinfo` does not look at the disk | Yes. IntelliJ loses the stage entry; Ramify keeps it | Ramify is kept |
| 4 | Failure while restoring | Only a warning in the log (`LOG.warn`) | `Restore` error with paths, mode and hash | Yes, only on that failure | Ramify is kept |
| 5 | Prior reading | Only `ls-files` of the partial paths | `ls-files -s` and `diff --diff-filter=A` of all the affected paths, before touching anything | No | It is what allows points 1 and 2 |
| 6 | Partial and reading of the stage | Partial first | Reads first; writes the partial after. The partial paths belong to the selection, so what is excluded does not change | No | Kept |
| 7 | `update-index` of the partial | Without `--add` | With `--add` | Only with new files, which IntelliJ does not treat as partial | Kept: it is what the plan asks for |
| 8 | Mode of the partial | First that of the previous path of the rename | That of the path in the index and, without an entry, that of the disk | Not in the ordinary rename: the disk keeps the permission | Corrected with respect to the plan, which only looked at `ls-files` |
| 9 | Remove and add | Removes and then adds | Same. The plan said the opposite | Yes with the plan: `add` of `A.txt` kept `a.txt` and the deletion left the commit without the file | Corrected: IntelliJ's order, with a test |
| 10 | Forced `add` | `addPathsForce` | `add --ignore-errors -f` | Yes with the plan: a selected ignored file made the commit fail | Corrected, with a test |
| 11 | `rm --cached` | Without `-f` (unread), with `-r` | With `-f`, without `-r` (folders are not selected) | Yes without `-f`: git refuses if the stage differs from HEAD and from the disk | Corrected, with a test |
| 12 | `reset` | `reset -- <paths>` | `reset HEAD -- <paths>` and, without commits, `rm --cached --quiet -f` | No: see below | Task decision |
| 13 | Message encoding | Writes the file in `i18n.commitEncoding` | Always UTF-8, and `-c i18n.commitEncoding=UTF-8` in the commit | Yes without that `-c`: the commit was annotated as Latin-1 with UTF-8 bytes. With it, the text is correct, but the object goes in UTF-8 | Corrected, with a test |
| 14 | NFD names | No handling in these files | `-c core.precomposeunicode=false` in the index commands and in `add` of what is versioned | Yes: without it, `reset` did not remove an NFD entry from the stage and `add` created a second NFC entry next to the NFD one, in both cases exiting with 0 | Corrected, with tests. What is new continues with normal `add`: its name comes from the disk |
| 15 | Selected `git add -N` | Does not restore it | Becomes an intent to add again if the commit fails | Yes, only on failures | Ramify is kept |
| 16 | Excluded case-only rename | `git mv -f` when restoring | `--force-remove` and `--cacheinfo` | No | Kept: it does not touch the disk |
| 17 | Classification of the failure | Gpg and no changes | Plus `Hook` with its output | It does not change the repository | Requested by the task |
| 18 | Hook output | Goes through stderr: git redirects it | Same, and stdout and stderr are both kept just in case | No | Kept |
| 19 | `--date` | Commit option | Does not exist in `CommitOptions` | No interface uses it | Out of scope |
| 20 | Renames in a separate commit | `commitExplicitRenames`, according to a setting | Does not exist | No interface uses it | Out of scope |
| 21 | Own stage of a partial file | Replaces it and it stays in a loose blob | Same, but first saves a copy `before commit parcial`. Without a copy there is no commit | The commit does not change | Review decision |
| 22 | Simultaneous operations | The IDE's task queue | Lock per repository for commit, rollback, restoring a copy and recovering on open | Yes without it: two commits at once mixed their content | Review decision |
| 23 | The app dies during the commit | What was excluded does not go back to the stage | `commit-journal.json` in the repository's data, with the entry to restore and the one `unstage` leaves. `recover` restores on open or before the next operation, and only the paths that remain as `unstage` left them. What cannot come back is reported with mode and hash, and the journal is closed. With the index locked it stops and the journal is kept to retry | Yes, only in that case | Review decision |
| 24 | `git commit` outlives the app | No handling | `flock` of `commit.lock`, inherited as stdin of `git commit`. While it is still held, `recover` touches nothing and commit, rollback and restore fail with `commit_running` | Yes without it: restoring during a hook put what was excluded into that commit | Review decision. Tested with `kill -9` and a slow hook on git 2.38 and Apple git 2.54 |

## First commit and `git reset HEAD`

With git 2.38, `git reset HEAD -- <path>` works on a branch without commits: it exits with 0 and takes the empty tree. Also checked by removing the no-commits branch from `unstage`: the `primer_commit_*` tests keep passing.

`rm --cached` is kept by decision of the task, with `-f`, and `git::head_exists` is still there before touching anything. Without `-f`, `git rm --cached --quiet -- <path>` fails if the file changed on disk after being added, as `first_commit_of_the_repository` checks.

## Pending checks for Task 12

- The commands marked "(unread)", above all the exact options of `add` and whether `rm` carries `-f`.
- Hooks inherit `LC_ALL=C` and `GIT_OPTIONAL_LOCKS=0` from `git.rs`, and, when opened from Finder, the app does not have the terminal's `PATH`: a hook that uses `node` or `pnpm` may not find them. Compare with what IntelliJ does.
