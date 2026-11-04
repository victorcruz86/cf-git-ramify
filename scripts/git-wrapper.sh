#!/bin/bash
# Logs the arguments of every call to git and delegates to the real binary.
#
#   RAMA_GIT_LOG   log file; without it nothing is logged
#   RAMA_GIT_REAL  real git binary (defaults to /usr/bin/git)
#
# For rama to use it, link it as `git` in a folder at the start of the PATH.
# For IntelliJ, set it as the path of the Git executable.

if [ -n "$RAMA_GIT_LOG" ]; then
  {
    printf 'git'
    printf ' %q' "$@"
    printf '\n'
  } >>"$RAMA_GIT_LOG"
fi
exec "${RAMA_GIT_REAL:-/usr/bin/git}" "$@"
