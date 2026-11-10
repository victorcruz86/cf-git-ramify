#!/usr/bin/env python3
"""Runs the app's commands through the real IPC, in WebKit, against a throwaway repository.

Builds separately, with VITE_RAMA_SMOKE, and leaves the binary in target/smoke so as not to overwrite the release one.
Everything happens in a temporary folder, with its own HOME: it never touches a real repository or the app's
settings. The window is visible for a moment, so the screen has to be unlocked.

    python3 scripts/smoke_ipc.py --out docs/reports/validation-ipc.json
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TAURI = ROOT / "src-tauri"
# They go outside the window and the temporary folder, and the manual list covers them.
NOT_RUN = {
    "clipboard_write": "would write to the real clipboard",
    "open_in_new_window": "would open another instance outside the test's HOME",
    "github_accounts": "reads the real Keychain",
    "github_account_details": "asks GitHub for the account and reads the real Keychain",
    "github_login": "asks GitHub, and stores the token in the real Keychain",
    "github_remove": "deletes from the real Keychain",
    "open_url": "would open the real browser",
    "github_known": "reads no token; it belongs to the Pull Requests window, which the manual list covers, and the lab has no GitHub remote for it to find",
    "github_connect": "asks GitHub for the account and the repository, and reads the real Keychain",
    "github_pr_page": "asks GitHub for pull requests, and reads the real Keychain",
    "github_pr_etag": "asks GitHub for the list's ETag, and reads the real Keychain",
    "github_pr_chooser": "asks GitHub for collaborators, assignees or labels, and reads the real Keychain",
    "github_pr_details": "asks GitHub for a pull request, and reads the real Keychain",
    "github_pr_status": "asks GitHub for a pull request's status, and reads the real Keychain",
    "github_pr_commits": "asks GitHub for a pull request's commits, and reads the real Keychain",
    "github_pr_changes": "fetches from the remote and may ask GitHub for the commits, and reads the real Keychain",
    "github_pr_checkout": "may add a remote, fetches from it, may ask GitHub for the pull request, and reads the real Keychain",
    "github_pr_show_in_log": "may add a remote, fetches from it, may ask GitHub for the pull request, and reads the real Keychain",
}
LIMIT_S = 240


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def commit(repo, name, text, message):
    path = Path(repo) / name
    with open(path, "a") as out:
        out.write(text)
    git(repo, "add", name)
    git(repo, "commit", "-q", "-m", message)


def lab(base):
    """A repository with a merge, a tag, a remote, branches and uncommitted changes."""
    bare = base / "remote.git"
    repo = base / "repo"
    other = base / "other"
    git(base, "init", "-q", "--bare", "-b", "main", str(bare))
    git(base, "init", "-q", "-b", "main", str(repo))
    git(repo, "config", "user.name", "Test")
    git(repo, "config", "user.email", "test@example.com")
    git(repo, "remote", "add", "origin", str(bare))
    for name in "abcde":
        (repo / f"{name}.txt").write_text("".join(f"{name} {i}\n" for i in range(40)))
    git(repo, "add", ".")
    git(repo, "commit", "-q", "-m", "base")
    git(repo, "tag", "v1")
    git(repo, "checkout", "-q", "-b", "feature")
    commit(repo, "f.txt", "f\n", "feature")
    git(repo, "checkout", "-q", "main")
    commit(repo, "g.txt", "g\n", "main two")
    git(repo, "merge", "-q", "--no-ff", "-m", "merge feature", "feature")
    git(repo, "branch", "tracked2")
    git(repo, "push", "-q", "-u", "origin", "main", "feature", "tracked2", "v1")
    git(repo, "checkout", "-q", "-b", "gone", "feature")
    commit(repo, "h.txt", "h\n", "local only")
    git(repo, "checkout", "-q", "main")

    git(base, "clone", "-q", str(bare), str(other))
    git(other, "config", "user.name", "Other")
    git(other, "config", "user.email", "other@example.com")
    commit(other, "z.txt", "z\n", "incoming")
    git(other, "push", "-q", "origin", "main")
    git(other, "checkout", "-q", "-b", "extra")
    git(other, "push", "-q", "origin", "extra")

    (repo / ".git/info/exclude").unlink()
    for name in "abcde":
        with open(repo / f"{name}.txt", "a") as out:
            out.write("uncommitted change\n")
    (repo / "new.txt").write_text("new\n")
    return repo


def registered():
    text = (TAURI / "src/lib.rs").read_text()
    block = re.search(r"generate_handler!\[(.*?)\]\)", text, re.S).group(1)
    return {name.split("::")[-1] for name in re.findall(r"[\w:]+", block)}


def build():
    env = {**os.environ, "VITE_RAMA_SMOKE": "1"}
    subprocess.run(["pnpm", "build"], cwd=ROOT, env=env, check=True)
    target = TAURI / "target/smoke"
    subprocess.run(
        ["cargo", "build", "--release", "--features", "tauri/custom-protocol"],
        cwd=TAURI,
        env={**env, "CARGO_TARGET_DIR": str(target)},
        check=True,
    )
    # Leaves dist/ as a normal build expects it.
    subprocess.run(["pnpm", "build"], cwd=ROOT, check=True)
    return target / "release/rama"


def run(app, repo, home, report):
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(home),
        "RAMA_PERF_OUT": str(report),
        "GIT_CONFIG_NOSYSTEM": "1",
    }
    process = subprocess.Popen([str(app), str(repo)], env=env)
    limit = time.time() + LIMIT_S
    try:
        while time.time() < limit and process.poll() is None:
            if report.exists() and report.stat().st_size:
                return json.loads(report.read_text())
            time.sleep(0.5)
    finally:
        process.terminate()
        process.wait()
    sys.exit("The app did not deliver the report")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out")
    parser.add_argument("--app", help="Binary already built with VITE_RAMA_SMOKE")
    args = parser.parse_args()
    app = Path(args.app) if args.app else build()

    base = Path(tempfile.mkdtemp(prefix="rama-smoke-")).resolve()
    try:
        home = base / "home"
        home.mkdir()
        result = run(app, lab(base), home, base / "report.json")
    finally:
        shutil.rmtree(base, ignore_errors=True)
    if "refused" in result:
        sys.exit(f"The app refused to use {result['refused']}")

    steps = result["steps"]
    seen = {step["command"] for step in steps}
    missing = registered() - seen - set(NOT_RUN)
    ipc = [step for step in steps if step["ipc"]]
    app_errors = [step for step in steps if not step["ok"] and not step["ipc"]]
    for step in steps:
        mark = "ok " if step["ok"] else ("IPC" if step["ipc"] else "err")
        print(f"{mark} {step['command']:32} {step['ms']:6} ms  {step['detail'][:150]}")
    print(f"\n{len(seen)} commands of {len(registered())}, {len(steps)} steps")
    print(f"IPC failures: {len(ipc)}. Operation errors: {len(app_errors)}")
    print(f"Not run: {sorted(missing)}. Excluded on purpose: {sorted(NOT_RUN)}")
    if args.out:
        Path(args.out).write_text(json.dumps(result, ensure_ascii=False, indent=1) + "\n")
    sys.exit(1 if ipc or missing else 0)


if __name__ == "__main__":
    main()
