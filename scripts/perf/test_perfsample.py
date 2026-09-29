import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import perfsample  # noqa: E402

HERE = os.path.dirname(__file__)


class ParseCputime(unittest.TestCase):
    def test_minutes_seconds(self):
        self.assertAlmostEqual(perfsample.parse_cputime("0:01.23"), 1.23)
        self.assertAlmostEqual(perfsample.parse_cputime("12:00.50"), 720.5)

    def test_hours(self):
        self.assertAlmostEqual(perfsample.parse_cputime("1:02:03.00"), 3723.0)


class Summarize(unittest.TestCase):
    def test_mean_p95_max(self):
        rows = [(i, float(i), 100.0 + i) for i in range(1, 21)]  # cpu 1..20
        s = perfsample.summarize(rows)
        self.assertAlmostEqual(s["cpu_mean_pct"], 10.5)
        self.assertAlmostEqual(s["cpu_p95_pct"], 19.0)
        self.assertAlmostEqual(s["rss_max_mb"], 120.0)

    def test_empty_is_error(self):
        with self.assertRaises(ValueError):
            perfsample.summarize([])


class SampleCli(unittest.TestCase):
    def test_samples_a_live_process(self):
        p = subprocess.Popen(["sleep", "30"])
        try:
            with tempfile.TemporaryDirectory() as d:
                out = os.path.join(d, "s.csv")
                r = subprocess.run([sys.executable, os.path.join(HERE, "perfsample.py"), "sample", str(p.pid), "3", out])
                self.assertEqual(r.returncode, 0)
                lines = open(out).read().strip().splitlines()
                self.assertEqual(lines[0], "t,cpu_pct,rss_mb")
                self.assertEqual(len(lines), 4)
        finally:
            p.kill()

    def test_sample_exits_when_process_dies(self):
        p = subprocess.Popen(["sleep", "1"])
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "s.csv")
            r = subprocess.run(
                [sys.executable, os.path.join(HERE, "perfsample.py"), "sample", str(p.pid), "5", out],
                capture_output=True, text=True,
            )
            self.assertEqual(r.returncode, 1)
            self.assertIn("exited", r.stderr)


if __name__ == "__main__":
    unittest.main()
