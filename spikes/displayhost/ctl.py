#!/usr/bin/env python3
"""Hybrid display-host controller (spike). Models what the product supervisor will do:

  in-place  : `mode` on the running host — WindowServer's default is the wanted variant —
              seamless, keeps the display id. Works when WindowServer's default lands
              on the wanted variant.
  replace   : otherwise kill the host and start a fresh one with the same serial that
              selects the mode explicitly. An explicitly-selected display's mode list is
              frozen, so every later re-mode on that host also goes through replace.

Usage: ctl.py <count> [seed]  — random re-mode sequence, prints per-step outcome + summary.
"""
import os, random, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
SIZES = [(3440, 1440), (2560, 1440), (1920, 1080), (1714, 1288), (1714, 1287), (1600, 1200),
         (1280, 720), (2000, 1200), (1200, 900), (2560, 1600), (1920, 1200), (1366, 768)]


class Host:
    def __init__(self, serial, pw, ph, scale):
        env = dict(os.environ)
        self.p = subprocess.Popen([os.path.join(HERE, "displayhost")], stdin=subprocess.PIPE,
                                  stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)
        self.reply = self.cmd(f"create {serial} {pw} {ph} {scale}")

    def cmd(self, line):
        self.p.stdin.write(line + "\n"); self.p.stdin.flush()
        return self.p.stdout.readline().strip()

    def kill(self):
        """Graceful destroy: the host removes its display under the machine-wide lock."""
        try:
            self.p.stdin.write("quit\n"); self.p.stdin.flush()
        except BrokenPipeError:
            pass
        self.p.wait()


def want_ok(reply, pw, ph, scale):
    f = reply.split()
    return len(f) == 6 and f[0] == "ok" and (int(f[2]), int(f[3]), int(f[4]), int(f[5])) == (pw // scale, ph // scale, pw, ph)


def main():
    n = int(sys.argv[1]); rnd = random.Random(int(sys.argv[2]) if len(sys.argv) > 2 else None)
    os.environ["DH_VENDOR"] = "0x%08X" % rnd.getrandbits(32)
    serial = 9000 + rnd.randrange(1000)
    stats = {"inplace": 0, "replace": 0, "fail": 0}
    t_in, t_rep = [], []
    host = Host(serial, 3440, 1440, 1)
    assert want_ok(host.reply, 3440, 1440, 1), host.reply
    for i in range(n):
        w, h = rnd.choice(SIZES); scale = rnd.choice([1, 2])
        pw, ph = w * scale, h * scale
        t0 = time.time(); how = None
        r = host.cmd(f"mode {pw} {ph} {scale}")
        if want_ok(r, pw, ph, scale):
            how = "inplace"; t_in.append(time.time() - t0)
        elif r == "err needs-replace":
            host.kill(); host = Host(serial, pw, ph, scale)
            how = "replace" if want_ok(host.reply, pw, ph, scale) else "fail"
            if how == "replace": t_rep.append(time.time() - t0)
        else:
            how = "fail"; host.reply = r
        stats[how] += 1
        print(f"{i+1:3} {pw}x{ph}@{scale}x {how:8} {time.time()-t0:5.2f}s  {host.reply if how != 'inplace' else ''}", flush=True)
    host.kill()
    avg = lambda a: sum(a) / len(a) if a else 0
    print(f"SUMMARY n={n} inplace={stats['inplace']} replace={stats['replace']} fail={stats['fail']} "
          f"avg_inplace={avg(t_in):.2f}s avg_replace={avg(t_rep):.2f}s max_replace={max(t_rep, default=0):.2f}s")


if __name__ == "__main__":
    main()
