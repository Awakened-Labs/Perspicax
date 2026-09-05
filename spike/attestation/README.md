# The Wine attestation spike

**The question.** Can a Wine window's identity come from two kernel-attested
sources that do not both reduce to wineserver's bookkeeping?

**The budget.** One day. Against stock sway, never against `wm`. SO_PEERCRED
attestation is compositor-independent, so involving our own compositor would add
a variable and answer nothing; and "does `winewayland.drv` connect to `wm`" is a
separate protocol-surface question that must not eat this one.

**The deliverable.** Strong, Weak, or Dishonest-only — one paragraph, with the
mechanism.

## Why a proxy and not a compositor

`WAYLAND_DISPLAY` points at a socket `wl-attest` owns. It accepts, records what
the kernel says about the peer, connects upstream to a stock sway, and forwards.
Clients cannot tell. This is what makes the measurement compositor-independent
in fact rather than in principle.

One thing a byte proxy must get right: Wayland passes file descriptors as
`SCM_RIGHTS` — shm pools, dmabuf, xkb keymaps. A `splice()` proxy drops them and
the client dies at its first buffer. Every message is `recvmsg`/`sendmsg`'d with
its fds carried across.

The proxy deliberately does **not** parse the wire protocol. Object ids, app_ids
and titles come from `WAYLAND_DEBUG=1` on the client, whose output is already
per-process; the ledger carries a byte offset so the two join after the fact.
Parsing only becomes necessary if send-time and connect-time credentials are
ever seen to disagree (M2), which they were not.

## The pieces

| | |
|---|---|
| `wl-attest.c` | the proxy: `SO_PEERCRED` at accept, `SO_PASSCRED` per message, JSONL ledger |
| `fdpass-probe.c` | M2's **negative control** — manufactures a divergence on purpose |
| `attest-win.c` | Win32 top-levels in the shapes the measurements need (mingw, stock PE) |
| `secctx-launch.c` | runs a command behind a `wp_security_context_v1` socket (M5) |
| `census.py` | reads the ledger, answers M1–M4 |
| `m5-check.py` | reads sandbox attribution back out of sway |
| `run-spike.sh` | `preflight`, `prefix`, `up`, `control`, `measure`, `m5` |

```sh
make && ./run-spike.sh all
```

## The two sources under test

- **A — `SO_PEERCRED`.** Who called `connect()`. Taken once, at accept, never
  refreshed.
- **B — `SCM_CREDENTIALS`** via `SO_PASSCRED`. Who called `sendmsg()`, stamped by
  the kernel on every message.

They are not one source stated twice: they diverge exactly when the connection
outlives the process that opened it. `fdpass-probe` shows it — `SO_PEERCRED`
names the parent forever while `SCM_CREDENTIALS` names the child from byte 12
on. Neither asks any userspace bookkeeper, which is the property under test.

Everything else derived from the pid — `exe`, `cgroup`, `/proc` generally — is a
*function of* A, so it corroborates rather than adds. `SO_PEERSEC` is not a
candidate on a box with no LSM active.

- **C — launcher provenance**, `wp_security_context_v1`. Rooted somewhere else
  entirely: a launcher creates the listening socket and stamps it *before Wine
  exists*, so it cannot reduce to wineserver's bookkeeping whatever wineserver
  later believes.

## Two traps that cost time here

**Arm `SO_PASSCRED` before `listen()`.** The kernel stamps a message only if the
receiving socket had the option when the message was queued. Set it after the
upstream `connect()` and the first requests arrive with `pid: 0` — which is not
"sent by pid 0" but "we were not listening yet", and it silently leaves the
earliest surface-creating requests unattested.

**Do not observe sandbox attribution with sway marks.** A sway mark is unique
across the tree, so `for_window [...] mark --add X` does not label a set — it
hands one label from window to window as each is created. It reports the last
match and looks exactly like "only one window was attributed". `m5-check.py`
moves matches to a named workspace instead, which is set-valued.

## Readings

Measured on this box — sway 1.11, Wine 11.4 staging (wow64), wayland 1.24,
kernel 6.12.58, no LSM, OpenRC — while verifying the harness. Recorded here as
the harness's own output, not as the verdict.

**M1 — one Wayland connection per Unix process.** Six connections, six distinct
pids, no pid holding two. Wine's fan-out (`explorer.exe /desktop`,
`winemenubuilder.exe`, the app, its spawned children) each get their own. The
failure mode that would have ended the spike at Dishonest-only — every window
arriving on one connection — did not occur.

**M2 — zero divergences** across all Wine connections, against a control that
proves the instrument can see one. So B corroborates A rather than naming
anything new: it is an integrity check against fd hand-off, not a second name.

**M3 — no kernel-side discriminator between windows of one process.** Three
top-levels, one connection, one pid, all asserting the same
`set_app_id("attest-win.exe")`, separated only by `set_title` — both
client-asserted.

**M4 — the executable is not attested.** `/proc/<pid>/exe` is
`.../wine-preloader` for *every* Wine client. The Windows program name appears
only in `cmdline`, and Wine demonstrably rewrites its own — the ledger's
`cmdline_unattested` shows Windows command lines padded out with trailing
spaces into the argv region. Win32 pid and Unix pid are unrelated numbers (32 vs
381575); the mapping is wineserver's.

**M5 — launcher provenance works and is coarse.** 4 of 4 windows attributed to
one `sandbox_app_id`, spanning 3 distinct Unix pids. One stamp covers every
process Wine spawned. Genuinely independent of A and B, and strictly coarser
than either.

## What remains

The verdict paragraph, and the judgement behind it: A and B pin a window to a
Unix *process* without wineserver, C names the *app instance* without
wineserver — but nothing kernel-side separates two windows of one process, and
nothing kernel-side says which Windows program a process is running. Whether
that is Weak, or Strong-conditional-on-owning-the-launcher, is the call the
paragraph has to make and defend.
