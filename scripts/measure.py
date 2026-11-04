#!/usr/bin/env python3
"""Measures the app's memory, CPU, startup and smoothness, or those of another process by PID."""

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time

IDLE_SECONDS = 30
CPU_WINDOW_SECONDS = 30
REPORT_TIMEOUT_SECONDS = 180
BYTES_IN_MB = 1024 * 1024
UNIT_BYTES = {"B": 1, "KB": 1024, "MB": BYTES_IN_MB, "GB": 1024 * BYTES_IN_MB}
FOOTPRINT_LINE = re.compile(r"phys_footprint:\s+([\d.]+)\s+(B|KB|MB|GB)\b")


def parse_footprint(text):
    match = FOOTPRINT_LINE.search(text)
    if not match:
        return None
    return int(float(match.group(1)) * UNIT_BYTES[match.group(2)])


def parse_cputime(text):
    seconds = 0.0
    for part in text.strip().split(":"):
        seconds = seconds * 60 + float(part)
    return seconds


def new_webkit_pids(before, after):
    return sorted(after - before)


def median_value(values):
    if isinstance(values[0], dict):
        keys = {key for value in values for key in value}
        return {key: median_value([value[key] for value in values if key in value]) for key in keys}
    known = [value for value in values if value is not None]
    if not known:
        return None
    if isinstance(known[0], bool):
        return all(known)
    if not isinstance(known[0], (int, float)):
        return known[0]
    return statistics.median(known)


# A run without a window does not paint: if any run is visible, the others stay out of the median.
def median_report(runs):
    visible = [run for run in runs if run.get("window_visible")]
    chosen = visible or runs
    report = {key: median_value([run[key] for run in chosen]) for key in runs[0]}
    if "window_visible" in runs[0]:
        report["window_visible"] = all(run["window_visible"] for run in runs)
        report["visible_runs"] = len(visible)
    return report


def read_report(path):
    if os.path.getsize(path) == 0:
        return None
    try:
        with open(path, encoding="utf-8") as report:
            return json.load(report)
    except json.JSONDecodeError:
        return None


def report_metrics(report, started_ms):
    origin = report["timeOriginEpochMs"]
    diff = report["diff"] or {}
    merge = report.get("merge") or {}
    visible = report.get("windowVisible", True)

    def since_launch(stage):
        mark = report["stages"].get(stage)
        return None if mark is None else origin + mark - started_ms

    return {
        "window_visible": visible,
        "startup_ms": report["readyAtEpochMs"] - started_ms if visible else None,
        "first_paint_ms": since_launch("first-paint"),
        "window_shown_ms": since_launch("window-shown"),
        "diff_path": diff.get("path"),
        "diff_error": diff.get("error"),
        "diff_open_ms": diff.get("openMs"),
        "diff_sides_ms": diff.get("sidesMs"),
        "diff_build_ms": diff.get("buildMs"),
        "diff_paint_ms": diff.get("paintMs"),
        "merge_path": merge.get("path"),
        "merge_error": merge.get("error"),
        "merge_open_ms": merge.get("openMs"),
        "frames": report["frames"],
        "p50": report["p50"],
        "p95": report["p95"],
        "p99": report["p99"],
        "over50ms": report["over50ms"],
        "stages": report["stages"],
    }


def process_table():
    output = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,command="], capture_output=True, text=True, check=True
    ).stdout
    rows = []
    for line in output.splitlines():
        pid, ppid, command = line.split(None, 2)
        rows.append((int(pid), int(ppid), command))
    return rows


def webkit_pids():
    return {pid for pid, _, command in process_table() if "com.apple.WebKit" in command}


def with_descendants(root):
    table = process_table()
    found = {root}
    grew = True
    while grew:
        grew = False
        for pid, ppid, _ in table:
            if ppid in found and pid not in found:
                found.add(pid)
                grew = True
    return found


def list_webkit(pids):
    commands = {pid: command for pid, _, command in process_table()}
    for pid in pids:
        print(f"WebKit counted: {pid} {commands.get(pid, '(terminated)')}", file=sys.stderr)


def footprint_bytes(pid):
    result = subprocess.run(
        ["footprint", "--noCategories", str(pid)], capture_output=True, text=True
    )
    value = parse_footprint(result.stdout)
    if value is None:
        sys.exit(f"footprint could not read process {pid}. Run this script again with sudo.")
    return value


def memory_mb(pids):
    return sum(footprint_bytes(pid) for pid in pids) / BYTES_IN_MB


def cpu_seconds(pids):
    result = subprocess.run(
        ["ps", "-o", "time=", "-p", ",".join(str(pid) for pid in pids)],
        capture_output=True,
        text=True,
    )
    return sum(parse_cputime(line) for line in result.stdout.splitlines() if line.strip())


def cpu_percent(pids):
    before = cpu_seconds(pids)
    time.sleep(CPU_WINDOW_SECONDS)
    return (cpu_seconds(pids) - before) / CPU_WINDOW_SECONDS * 100


def wait_for_report(path, process):
    deadline = time.time() + REPORT_TIMEOUT_SECONDS
    while time.time() < deadline:
        if process.poll() is not None:
            sys.exit(f"The app exited with code {process.returncode}")
        report = read_report(path)
        if report is not None:
            return report
        time.sleep(0.2)
    sys.exit("The app did not send the report in time")


def measure_run(app, repo):
    with tempfile.NamedTemporaryFile(suffix=".json") as report_file:
        before = webkit_pids()
        load_before = os.getloadavg()[0]
        started = time.time()
        process = subprocess.Popen(
            [app, repo], env={**os.environ, "RAMA_PERF_OUT": report_file.name}
        )
        try:
            report = wait_for_report(report_file.name, process)
            time.sleep(IDLE_SECONDS)
            webviews = new_webkit_pids(before, webkit_pids())
            list_webkit(webviews)
            pids = [process.pid, *webviews]
            return {
                "load_1m": load_before,
                "memory_mb": memory_mb(pids),
                "webview_mb": memory_mb(webviews),
                "cpu_percent": cpu_percent(pids),
                **report_metrics(report, started * 1000),
            }
        finally:
            process.terminate()
            process.wait()


def measure_pid(pid):
    pids = sorted(with_descendants(pid))
    return {"memory_mb": memory_mb(pids), "cpu_percent": cpu_percent(pids)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app")
    parser.add_argument("--repo")
    parser.add_argument("--pid", type=int)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    if args.pid:
        result = measure_pid(args.pid)
    else:
        runs = [measure_run(args.app, args.repo) for _ in range(args.runs)]
        result = {"median": median_report(runs), "runs": runs}

    with open(args.out, "w", encoding="utf-8") as out:
        json.dump(result, out, indent=2)
        out.write("\n")
    print(json.dumps(result.get("median", result), indent=2))


if __name__ == "__main__":
    main()
