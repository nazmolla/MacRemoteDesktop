#!/usr/bin/env python3
"""Sample one process's CPU% (from cumulative CPU time deltas, not ps's decaying
average) and RSS once per second; summarize a CSV of samples."""
import csv
import subprocess
import sys
import time


def parse_cputime(s: str) -> float:
    """ps 'time' field: [[H:]M]:S.ss → seconds."""
    total = 0.0
    for part in s.strip().split(":"):
        total = total * 60 + float(part)
    return total


def _read(pid: int):
    r = subprocess.run(["ps", "-o", "state=,time=,rss=", "-p", str(pid)], capture_output=True, text=True)
    fields = r.stdout.split()
    # A zombie (state Z) has exited but not been reaped yet; ps still lists it.
    if r.returncode != 0 or len(fields) != 3 or fields[0].startswith("Z"):
        return None
    return parse_cputime(fields[1]), int(fields[2]) / 1024.0


def sample(pid: int, seconds: int, out_path: str) -> int:
    prev = _read(pid)
    if prev is None:
        print(f"process {pid} not found", file=sys.stderr)
        return 1
    t_prev = time.monotonic()
    with open(out_path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["t", "cpu_pct", "rss_mb"])
        for i in range(1, seconds + 1):
            time.sleep(max(0.0, t_prev + 1.0 - time.monotonic()))
            cur = _read(pid)
            now = time.monotonic()
            if cur is None:
                print(f"process {pid} exited after {i - 1}s; results invalid", file=sys.stderr)
                return 1
            cpu = (cur[0] - prev[0]) / (now - t_prev) * 100.0
            w.writerow([i, f"{cpu:.2f}", f"{cur[1]:.1f}"])
            prev, t_prev = cur, now
    return 0


def summarize(rows):
    if not rows:
        raise ValueError("no samples")
    cpus = sorted(r[1] for r in rows)
    p95 = cpus[max(0, int(round(0.95 * len(cpus))) - 1)]
    return {
        "cpu_mean_pct": sum(cpus) / len(cpus),
        "cpu_p95_pct": p95,
        "rss_max_mb": max(r[2] for r in rows),
    }


def _load(path: str):
    with open(path) as f:
        return [(int(r["t"]), float(r["cpu_pct"]), float(r["rss_mb"])) for r in csv.DictReader(f)]


def main(argv) -> int:
    if len(argv) == 5 and argv[1] == "sample":
        return sample(int(argv[2]), int(argv[3]), argv[4])
    if len(argv) == 3 and argv[1] == "summarize":
        s = summarize(_load(argv[2]))
        print(" ".join(f"{k}={v:.2f}" for k, v in s.items()))
        return 0
    print("usage: perfsample.py sample <pid> <seconds> <out.csv> | summarize <csv>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
