#!/usr/bin/env python3
"""Read a wl-attest ledger and answer the spike's measurements directly.

The ledger is deliberately dumb -- connections and credential transitions, no
protocol -- so this is where the two halves are put together and where the
decision tree gets its inputs. Nothing here infers; every line printed is a
count of something recorded.
"""
import json
import sys
from collections import Counter


def load(path):
    out = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line:
                out.append(json.loads(line))
    return out


def main(path, wldebug=None):
    ev = load(path)
    conns = {e["conn"]: e for e in ev if e["ev"] == "connect"}
    closes = {e["conn"]: e for e in ev if e["ev"] == "close"}
    creds = [e for e in ev if e["ev"] == "send_cred"]

    print("== M1  connection census: one Wayland connection per Unix process? ==")
    for c, d in sorted(conns.items()):
        cl = closes.get(c, {})
        prog = d["cmdline_unattested"].strip().split("\\")[-1].split("/")[-1][:38]
        print(f"  conn {c:>2}  peercred_pid={d['peercred_pid']:<8} "
              f"start={d['starttime']:<12} msgs={cl.get('c2s_msgs','-'):<5} "
              f"fds={cl.get('c2s_fds','-'):<3} {prog}")
    pids = [d["peercred_pid"] for d in conns.values()]
    dupes = [p for p, n in Counter(pids).items() if n > 1]
    print(f"  -> {len(conns)} connections, {len(set(pids))} distinct pids"
          f"{'; SHARED pids: ' + str(dupes) if dupes else '; no pid holds two connections'}")

    print("\n== M4  is the executable kernel-attested? ==")
    exes = Counter(d["exe"] for d in conns.values())
    for e, n in exes.items():
        print(f"  {n:>2}x /proc/<pid>/exe = {e}")
    if len(exes) == 1 and len(conns) > 1:
        print("  -> every client has the SAME /proc/exe. The Windows program name appears")
        print("     ONLY in cmdline, which the process can rewrite. Not attested.")

    print("\n== M2  did send-time creds ever disagree with connect-time? ==")
    div = [e for e in creds if e["prev_pid"] != -1]
    zeros = [e for e in creds if e["pid"] == 0]
    for e in div:
        print(f"  conn {e['conn']}: pid {e['prev_pid']} -> {e['pid']} at byte {e['at_byte']} (msg {e['msg']})")
    if zeros:
        print(f"  !! {len(zeros)} messages stamped pid 0 -- SO_PASSCRED was armed late; fix before trusting M2")
    print(f"  -> {len(div)} divergences across {len(conns)} connections")
    if not div:
        print("     Source B corroborates A but never adds identity here: it is an")
        print("     integrity check against fd-passing, not a second name.")

    if wldebug:
        print("\n== M3  window granularity: toplevels per connection ==")
        titles, appids = [], []
        for line in open(wldebug, errors="replace"):
            if "set_title(" in line:
                titles.append(line.split('set_title("')[-1].split('"')[0])
            if "set_app_id(" in line:
                appids.append(line.split('set_app_id("')[-1].split('"')[0])
        print(f"  toplevels titled: {titles}")
        print(f"  app_ids asserted: {sorted(set(appids))}")
        if len(titles) > 1 and len(set(appids)) == 1:
            print("  -> N windows, ONE app_id, one connection, one pid. Both discriminators")
            print("     (title, app_id) are client-asserted. No kernel evidence separates them.")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("usage: census.py ledger.jsonl [wayland-debug.log]")
    main(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else None)
