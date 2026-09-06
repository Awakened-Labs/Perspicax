#!/usr/bin/env python3
"""M5: which windows did the compositor attribute to the sandbox identity?

sway exposes sandbox_engine / sandbox_app_id / sandbox_instance_id ONLY as
window criteria -- get_tree carries none of them. So attribution has to be read
back indirectly, and the choice of indirection matters:

  marks       WRONG. A sway mark is unique across the tree, so a
              `for_window [...] mark --add X` rule does not label a set, it
              hands one label from window to window as each is created. It
              reports the LAST match and looks exactly like "only one window
              was attributed".
  workspace   Right. Moving every match to a named workspace is set-valued, so
              N matches produce N windows on that workspace.

Usage: run with the criteria to test; it applies them at query time to every
window currently open, then reports the partition.
"""
import json
import subprocess
import sys

WS = "sandboxed-probe"


def tree():
    return json.loads(subprocess.run(["swaymsg", "-t", "get_tree"],
                                     capture_output=True, text=True).stdout)


def windows(node, ws=None, out=None):
    out = [] if out is None else out
    if node.get("type") == "workspace":
        ws = node.get("name")
    if node.get("pid") or node.get("app_id"):
        out.append({"title": node.get("name"), "app_id": node.get("app_id"),
                    "pid": node.get("pid"), "ws": ws})
    for c in node.get("nodes", []) + node.get("floating_nodes", []):
        windows(c, ws, out)
    return out


def main(criteria):
    before = windows(tree())
    subprocess.run(["swaymsg", '[%s] move to workspace %s' % (criteria, WS)],
                   capture_output=True, text=True)
    after = {(w["title"], w["pid"]): w for w in windows(tree())}

    print("  criteria: [%s]\n" % criteria)
    print("  {:<20} {:<18} {:<9} {}".format("title", "app_id", "unix pid", "attributed"))
    print("  " + "-" * 68)
    hits = []
    for w in before:
        now = after.get((w["title"], w["pid"]))
        got = bool(now and now["ws"] == WS)
        if got:
            hits.append(w)
        print("  {:<20} {:<18} {:<9} {}".format(
            str(w["title"])[:19], str(w["app_id"])[:17], str(w["pid"]),
            "YES" if got else "no"))

    pids = {w["pid"] for w in hits}
    print("\n  %d of %d windows attributed, spanning %d distinct unix pids"
          % (len(hits), len(before), len(pids)))
    if len(hits) > 1 and len(pids) > 1:
        print("  -> ONE sandbox identity spans SEVERAL processes and SEVERAL windows.")
        print("     The launcher's stamp names the app INSTANCE. It is rooted in socket")
        print("     provenance, not in any pid, so it is genuinely independent of")
        print("     SO_PEERCRED -- and strictly coarser than it.")
    elif len(hits) > 1 and len(pids) == 1:
        print("  -> attributed windows all share one pid; no fan-out was captured.")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else 'sandbox_app_id="com.example.Wine"')
