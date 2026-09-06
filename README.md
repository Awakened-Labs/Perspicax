# perspicax — an agent-native window manager

A Wayland compositor that exposes everything a human can see on screen as a
typed, addressable API, so an agent can drive programs that have no API of
their own.

**Status: v1.** It observes, attributes, judges — and acts, through the
compositor's own seat, returning a receipt for what happened and a refusal that
names the surface in the way when it will not. See `Milestones` below.

## Why this lives in the compositor

Reading the screen for an agent is usually done from outside — a library asks
each application, over the accessibility bus, what it believes it has. That
works until it matters, and then it fails in three ways that cannot be fixed
from outside:

- **It does not know what is visible.** Z-order, occlusion and damage live in
  the compositor. An agent clicking a button that another window is covering
  is, from outside, an unfixable failure mode.
- **It does not know who drew it.** A universal screen API makes every rendered
  pixel an instruction channel. Only the compositor can say *this text came
  from surface S, owned by PID P, of origin O* — and provenance you cannot
  attach is provenance you cannot enforce.
- **It is too slow.** AT-SPI2 costs a D-Bus round trip per property, so a full
  tree read is measured in seconds. A cache that lives beside the damage
  events invalidating it turns an O(tree) poll into an O(changed) push.

So the semantic index lives *inside* the compositor, and acting happens through
the compositor's own input path — which is what makes focus, grabs and z-order
correct by construction rather than by hope.

## Shape

Two traits carry the design, and they are the reason this is not welded to one
compositor:

- **`Ingest`** — where nodes come from. `perspicax-atspi` reads stock GTK and Qt over
  the accessibility bus today; a faster path can replace it without the layers
  above noticing.
- **`HostView`** — what only a compositor knows: is this rect visible, who owns
  this surface, dispatch this input. `perspicax-compositor` implements it on Smithay;
  a GNOME extension or KWin plugin later is a port, not a rewrite.

`perspicax-index` sits between them and imports neither Wayland nor D-Bus.

```
perspicax              the composition root — one binary, `perspicax --headless`
perspicax-mcp          MCP server (rmcp, stdio) — six tools, DTOs, receipts  [portable]
perspicax-index        node cache, stable ids, selectors, deltas, refusals  [portable]
perspicax-node         node schema — AccessKit types plus Origin and Visibility
perspicax-atspi        impl Ingest — AT-SPI2 over D-Bus
perspicax-compositor   impl HostView — Smithay: outputs, seat, damage. Draws nothing.
perspicax-probe        dev CLI — dump a tree, time a read, explain a refusal
```

## Milestones

| | | |
|---|---|---|
| **M0** | Skeleton, conventions, CI | done |
| **M1** | Semantic index + AT-SPI ingest — no compositor, runs on X11 | done |
| **M2** | Compositor, provenance, occlusion, damage-driven invalidation | done |
| **M3** | MCP server and act — **v1** | done |

v1 is one demo, run against a GTK app and a Qt app, headless, in CI, with zero
screenshots taken: observe a window, resolve a selector, click it, get a
receipt — and get a *refusal* when the target is occluded. It is
`crates/perspicax/tests/act.rs`.

## What reading a tree costs

Measured on Debian 13 (GNOME's accessibility stack, at-spi2-core 2.56.2), best
of five, with `perspicax-probe time --app <name>`. These numbers are the argument for
ever building a faster ingest path, so they are measured rather than asserted.

| | GTK 4.18.6 <br> `gtk4-widget-factory`, 278 nodes | Qt 6.8.2 <br> `gallery`, 219 nodes |
|---|---|---|
| first read, cold | 5.73 s | 2.74 s |
| `Cache.GetItems` | **62.3 ms** | 4.0 ms — **and 0 nodes** |
| recursive walk | 5.25 s | 1.86 s |
| walk + geometry | 5.96 s | 3.21 s |
| drain one delta | **2 µs** | **2 µs** |

Three things in that table matter more than the absolute figures.

**The fast path is ~84× the walk, and only GTK has one.** Qt 6.8.2 exports
`org.a11y.atspi.Cache`, introspects cleanly, declares `GetItems`, and answers
it with an empty array — a successful reply containing nothing. Probing by
interface therefore takes the fast path, receives no nodes, and reports that a
window full of widgets is empty. `perspicax-atspi` probes by *result* for this reason.

**A cold cache is not a small cache, it is a different answer.** GTK's is
filled by ATK as accessibles are realised, so a freshly started
`gtk4-widget-factory` answers `GetItems` with **11** of its 278 nodes — again
successfully, with nothing in the reply to suggest anything is missing. Walk it
once and the cache holds all 278 thereafter. `perspicax-atspi` compares each item's
declared child count against the children actually delivered and falls back to
the walk on any shortfall, which is why the `first read` row above says 5.73 s
and not 62 ms: it walked, because the cache asked to be doubted.

**Staying current costs six orders of magnitude less than starting over.** A
drain reads signals that already arrived and asks the application nothing. That
gap — 2 µs against seconds — is the whole reason this project treats polling a
tree as the bug rather than the fallback.

**`CoordType::Screen` is unusable, and not only under Wayland.** Asking both
applications for screen-relative and window-relative extents under a GNOME Xorg
session and a GNOME Wayland session: Qt reports true screen offsets on Xorg and
degrades to window-relative under Wayland, where a client genuinely cannot know
where it is. GTK answers `Screen` with coordinates that contradict its own
`Window` answer on *both* — `0,0` for a child it simultaneously places at
`10,57`. This crate therefore only ever asks for `Window`, and only a compositor
may turn those into anything global.

Geometry is a separate row because no bulk API exists for it on either toolkit:
`Cache.GetItems` carries roles, names, states and parentage and no extents at
all, so bounds are a round trip per node even on the fast path.

## What the compositor adds

M2's claim in one command, which is also `crates/perspicax/tests/demo.rs`:

```sh
perspicax --headless --spawn gtk4-widget-factory --spawn gallery --dump-tree 8
```

It starts an accessibility registry, hosts both toolkits as Wayland clients of
its own compositor, reads their trees, and binds each window to the surface it
was drawn on. Then every node has the two fields the schema has carried as
`Unknown` since the first commit:

```
gallery (Qt) -- 219 nodes
  window 2 "Widget Gallery Qt 6.8.2" -> surface 1  [Pid(10478), Title]
  219 judged: 2 visible, 122 occluded (0 unproven), 94 clipped
gtk4-widget-factory (GTK) -- 275 nodes
  window 2 "GTK Widget Factory" -> surface 2  [Pid(10479), Title]
  275 judged: 263 visible, 0 occluded, 1 clipped
```

The gallery reads 124 visible and 0 occluded when it is alone on the screen.
Placing the GTK window over it moves 122 of those nodes to `Occluded`, each
naming the surface in the way, and the refusal gate stops every one of them —
which is the failure mode no library outside a compositor can even detect.

**The process is the gate and the title is not.** A window binds to a surface
when two separately attested pids agree: one from the Wayland connection's
credentials, one from the accessibility bus daemon. A title that matches is
extra evidence and never sufficient — on the test bed, `mutter-x11-frames`
draws X11 decorations and therefore truthfully advertises the string
"Widget Gallery Qt 6.8.2" from a different process. A join that trusted titles
would have attributed the gallery's entire tree to the window manager.
Everything that cannot be bound stays `Unattributed` and is refused, and the
reason is reported rather than logged.

## What acting looks like

```sh
perspicax --headless --mcp --spawn gtk4-widget-factory
```

That is an MCP server on stdin and stdout with a compositor behind it. Six
tools: `window_list`, `observe`, `resolve`, `act`, `deltas`, `screenshot`.

An agent names a control and never a coordinate — the rectangle comes from the
index and turning it into anything global is the compositor's job, so an agent
that cannot name a pixel cannot name the wrong one. The input then goes onto the
same `wl_pointer` and `wl_keyboard` a real device would use, which is what makes
focus, grabs and z-order correct by construction rather than by a convention
every client has to honour.

**An act returns evidence, not a verdict.** There is no `success` field,
because none could be honest:

```json
{ "selector": "Button:Cancel", "node": 6, "surface": 1,
  "rendered_by": { "pid": 4242, "exe": "/usr/bin/gtk4-widget-factory" },
  "verb": "click", "dispatch_ms": 0.15,
  "focus_before": 1, "focus_after": 1, "focus_landed": true,
  "damage": { "witness": "on_target", "frames": 1, "window_ms": 200 } }
```

`damage` says what the pixels did in the 200 ms the act was given, scoped to the
target's own rectangle. Weigh it against the idle rates in the table below: an
idle GTK window repaints about forty times a second, so `on_target` means less
there than it does on Qt, and `quiet` — nothing changed at all — is the strong
answer on both. A boolean would have been confidently wrong on one of the two
toolkits this project tests against.

**A refusal is an answer.** It names what is in the way and what would clear it,
so a recoverable situation does not become a retry loop:

```json
{ "kind": "occluded", "message": "node occluded by surface 2", "occluded_by": 2 }
```

`observe` reports the same verdict per node, as `actable` plus `refused`, so an
agent sees an occlusion before it spends a call discovering one.

**Text an application rendered is marked as such.** Every string reaches an
agent under `untrusted_text`, beside the credentials of the process that drew
it — the marking is in the key, so it cannot be skimmed past, and the provenance
is at the point of use rather than in a preamble a model has to have remembered.
That is this project's injection defence, and it is a read-path property rather
than an act-path gate.

**There is no capability gate, deliberately.** perspicax is one actuator among several,
not an agent's only one: an agent refused a click runs the command instead, so a
gate here would document an intention rather than enforce a boundary — and a
control that can be trivially bypassed is worse than an absent one, because it
invites reliance. `Refusal::NoCapability` is declared and unconstructed until
`--seat`, where perspicax will host applications the user launched rather than ones it
spawned, and the question finally has two different answers.

**`screenshot` ships declared and always refusing.** There is no renderer in
this build at all — occlusion needs geometry, z-order, regions and damage, and
none of those need pixels. It is listed so that a model can tell the fallback
from the mechanism, and its refusal reports the count of nodes under rendering
no semantic event explained, which is the only honest trigger for a pixel path
and a number the compositor already computes.

## What a toolkit renders without explaining

Measured on the same Debian 13 box, hosting both applications under
`perspicax --headless` for eight idle seconds with nobody touching them.

| | GTK 4.18.6 <br> `gtk4-widget-factory` | Qt 6.8.2 <br> `gallery` |
|---|---|---|
| frames of damage, idle | **768** | 18 |
| nodes under damage no a11y event explained | 263 of 275 | 12 of 219 |

The first row is the one that changes a design. An idle GTK application repaints
its whole window about forty times a second while nothing is happening, so a
rule that treated surface damage as making a node stale would refuse every node
in that application permanently — and no re-read is fast enough to recover,
because reading its tree costs 62 ms at best. Damage therefore cannot mean
"unsafe to act"; what it means is *pixels changed here and the semantic feed did
not mention it*.

That is the fused signal this project exists to notice, and these numbers are
its baseline. GTK and Qt both explain themselves, so their unexplained repaints
are exactly that — repaints. A Flutter, GL or canvas surface produces the same
signal for a different reason: it renders and no bridge can say what it drew.
Telling those two apart is what a compositor is for, and it is the only honest
trigger for a vision fallback.

## Build

```sh
cargo build --workspace
cargo test  --workspace
```

Gates, in the order CI runs them:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny --all-features check
cargo test --workspace
```

Tests that need a real accessibility bus and real applications are `#[ignore]`d,
so the four gates above stay green on a machine with no graphical session. To
run them:

```sh
export XDG_RUNTIME_DIR=/run/user/$(id -u)
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
cargo test -p perspicax-atspi --test live -- --ignored --test-threads=1
cargo test -p perspicax --test act  -- --ignored --test-threads=1
```

The second is v1's exit criterion, and it is the one command that asserts the
whole claim: two toolkits hosted, a control clicked in each with a receipt to
show for it, and a covered control refused with the occluding surface named.

Without those two variables an SSH session finds an empty desktop and reports
no error worth reading. `perspicax-probe apps` says what the bus can actually see.

`scripts/provision-testbed.sh` builds that machine from a fresh Debian 13
install — the desktop, both toolkits, the bus and the pinned toolchain — for
anyone reproducing the figures above rather than taking them on trust.
`ci/live-tests.sh` is the other half: the same suite with no desktop at all.

## Licence

Apache-2.0. See `LICENSE`.
