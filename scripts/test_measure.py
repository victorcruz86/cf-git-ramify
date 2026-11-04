import os
import tempfile
import unittest

from measure import (
    median_report,
    new_webkit_pids,
    parse_cputime,
    parse_footprint,
    read_report,
    report_metrics,
)


class ParseFootprintTest(unittest.TestCase):
    def test_parse_footprint(self):
        text = "Auxiliary data:\n    phys_footprint: 2304 KB\n    phys_footprint_peak: 2320 KB\n"
        self.assertEqual(parse_footprint(text), 2359296)

    def test_other_units(self):
        self.assertEqual(parse_footprint("phys_footprint: 12 B"), 12)
        self.assertEqual(parse_footprint("phys_footprint: 1.5 MB"), 1572864)
        self.assertEqual(parse_footprint("phys_footprint: 2 GB"), 2147483648)

    def test_output_without_data(self):
        self.assertIsNone(parse_footprint("footprint: permission denied"))


class ProcessTest(unittest.TestCase):
    def test_new_processes(self):
        self.assertEqual(new_webkit_pids({1, 2}, {2, 3, 4}), [3, 4])


class CpuTimeTest(unittest.TestCase):
    def test_minutes_and_seconds(self):
        self.assertEqual(parse_cputime("1:02.50"), 62.5)

    def test_hours(self):
        self.assertEqual(parse_cputime("1:00:00.00"), 3600.0)


class MedianTest(unittest.TestCase):
    def test_median_per_field(self):
        runs = [{"memory_mb": 300}, {"memory_mb": 100}, {"memory_mb": 200}]
        self.assertEqual(median_report(runs), {"memory_mb": 200})


    def test_stages_per_key(self):
        runs = [{"stages": {"a": 1, "b": 9}}, {"stages": {"a": 3, "b": 7}}, {"stages": {"a": 2}}]
        self.assertEqual(median_report(runs), {"stages": {"a": 2, "b": 8}})

    def test_fields_without_data_do_not_break_the_median(self):
        runs = [{"diff_open_ms": None}, {"diff_open_ms": 10}, {"diff_open_ms": 20}]
        self.assertEqual(median_report(runs), {"diff_open_ms": 15})
        self.assertEqual(median_report([{"diff_open_ms": None}]), {"diff_open_ms": None})

    def test_non_numeric_fields_are_copied(self):
        runs = [{"diff_path": "a.c"}, {"diff_path": "a.c"}]
        self.assertEqual(median_report(runs), {"diff_path": "a.c"})

    def test_without_any_visible_window_uses_all(self):
        runs = [
            {"window_visible": False, "memory_mb": 100},
            {"window_visible": False, "memory_mb": 300},
        ]
        self.assertEqual(
            median_report(runs), {"window_visible": False, "memory_mb": 200, "visible_runs": 0}
        )

    def test_with_mixed_runs_only_visible_ones_count(self):
        runs = [
            {"window_visible": False, "memory_mb": 100},
            {"window_visible": True, "memory_mb": 300},
            {"window_visible": True, "memory_mb": 500},
        ]
        self.assertEqual(
            median_report(runs), {"window_visible": False, "memory_mb": 400, "visible_runs": 2}
        )

    def test_all_visible(self):
        runs = [{"window_visible": True, "memory_mb": 100}, {"window_visible": True, "memory_mb": 300}]
        self.assertEqual(
            median_report(runs), {"window_visible": True, "memory_mb": 200, "visible_runs": 2}
        )


class ReadReportTest(unittest.TestCase):
    def write(self, text):
        report = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
        self.addCleanup(os.unlink, report.name)
        report.write(text)
        report.close()
        return report.name

    def test_incomplete_report_is_retried(self):
        self.assertIsNone(read_report(self.write('{"readyAtEpochMs": 12')))

    def test_empty_report_is_retried(self):
        self.assertIsNone(read_report(self.write("")))

    def test_complete_report(self):
        self.assertEqual(read_report(self.write('{"frames": 3}')), {"frames": 3})


REPORT = {
    "readyAtEpochMs": 5000,
    "timeOriginEpochMs": 1200,
    "stages": {"first-paint": 800, "ready": 3800},
    "frames": 900,
    "p50": 10,
    "p95": 11,
    "p99": 19,
    "over50ms": 1,
    "diff": {
        "path": "a.c",
        "openMs": 300,
        "sidesMs": 40,
        "buildMs": 90,
        "paintMs": 16,
        "builtAtEpochMs": 9000,
    },
}


class ReportMetricsTest(unittest.TestCase):
    def test_keeps_stages(self):
        metrics = report_metrics(REPORT, 1000)
        self.assertEqual(metrics["stages"], REPORT["stages"])

    def test_startup_since_launch(self):
        metrics = report_metrics(REPORT, 1000)
        self.assertEqual(metrics["startup_ms"], 4000)
        self.assertEqual(metrics["first_paint_ms"], 1000)

    def test_merge_window_measurement(self):
        report = {**REPORT, "merge": {"path": "builtin/commit.c", "openMs": 120}}
        metrics = report_metrics(report, 1000)
        self.assertEqual(metrics["merge_path"], "builtin/commit.c")
        self.assertEqual(metrics["merge_open_ms"], 120)
        self.assertIsNone(metrics["merge_error"])

    def test_without_merge_the_fields_stay_empty(self):
        for report in (REPORT, {**REPORT, "merge": None}):
            metrics = report_metrics(report, 1000)
            self.assertIsNone(metrics["merge_path"])
            self.assertIsNone(metrics["merge_open_ms"])

    def test_failure_to_open_the_merge(self):
        report = {**REPORT, "merge": {"path": "a.c", "error": "could not open"}}
        self.assertEqual(report_metrics(report, 1000)["merge_error"], "could not open")

    def test_diff_timings(self):
        metrics = report_metrics(REPORT, 1000)
        self.assertEqual(metrics["diff_path"], "a.c")
        self.assertEqual(metrics["diff_open_ms"], 300)
        self.assertEqual(metrics["diff_sides_ms"], 40)
        self.assertEqual(metrics["diff_build_ms"], 90)
        self.assertEqual(metrics["diff_paint_ms"], 16)

    def test_window_shown(self):
        stages = {**REPORT["stages"], "window-shown": 500}
        metrics = report_metrics({**REPORT, "stages": stages}, 1000)
        self.assertEqual(metrics["window_shown_ms"], 700)

    def test_without_visible_window_there_is_no_startup_or_first_paint(self):
        stages = {"window-shown": 500, "ready": 19000}
        report = {**REPORT, "stages": stages, "windowVisible": False}
        metrics = report_metrics(report, 1000)
        self.assertFalse(metrics["window_visible"])
        self.assertIsNone(metrics["startup_ms"])
        self.assertIsNone(metrics["first_paint_ms"])
        self.assertEqual(metrics["window_shown_ms"], 700)

    def test_without_diff(self):
        metrics = report_metrics({**REPORT, "diff": None}, 1000)
        self.assertIsNone(metrics["diff_open_ms"])
        self.assertIsNone(metrics["diff_error"])

    def test_a_diff_failure_is_kept(self):
        report = {**REPORT, "diff": {"path": "a.c", "error": "could not open"}}
        metrics = report_metrics(report, 1000)
        self.assertEqual(metrics["diff_path"], "a.c")
        self.assertEqual(metrics["diff_error"], "could not open")
        self.assertIsNone(metrics["diff_open_ms"])


if __name__ == "__main__":
    unittest.main()
