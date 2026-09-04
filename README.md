# wm — an agent-native window manager

A Wayland compositor that exposes everything a human can see on screen as a
typed, addressable API, so an agent can drive programs that have no API of
their own.

**Status: pre-alpha.** The skeleton and conventions are in place; nothing
observes anything yet. See `Milestones` below for what exists.

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

- **`Ingest`** — where nodes come from. `wm-atspi` reads stock GTK and Qt over
  the accessibility bus today; a faster path can replace it without the layers
  above noticing.
- **`HostView`** — what only a compositor knows: is this rect visible, who owns
  this surface, dispatch this input. `wm-compositor` implements it on Smithay;
  a GNOME extension or KWin plugin later is a port, not a rewrite.

`wm-index` sits between them and imports neither Wayland nor D-Bus.

```
wm-mcp          MCP server (rmcp, stdio) — tools, receipts, capability gate
wm-index        node cache, stable ids, selectors, deltas, refusals  [portable]
wm-node         node schema — AccessKit types plus Origin and Visibility
wm-atspi        impl Ingest — AT-SPI2 over D-Bus
wm-compositor   impl HostView — Smithay: outputs, seat, xwayland, damage
wm-probe        dev CLI — dump a tree, time a read, explain a refusal
```

## Milestones

| | | |
|---|---|---|
| **M0** | Skeleton, conventions, CI | done |
| **M1** | Semantic index + AT-SPI ingest — no compositor, runs on X11 | in progress |
| **M2** | Compositor, provenance, occlusion, damage-driven invalidation | |
| **M3** | MCP server and act — **v1** | |

v1 is one demo, run against a GTK app and a Qt app, headless, in CI, with zero
screenshots taken: observe a window, resolve a selector, click it, get a
receipt — and get a *refusal* when the target is occluded.

## What reading a tree costs

Measured on Debian 13 (GNOME's accessibility stack, at-spi2-core 2.56.2), best
of five, with `wm-probe time --app <name>`. These numbers are the argument for
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
window full of widgets is empty. `wm-atspi` probes by *result* for this reason.

**A cold cache is not a small cache, it is a different answer.** GTK's is
filled by ATK as accessibles are realised, so a freshly started
`gtk4-widget-factory` answers `GetItems` with **11** of its 278 nodes — again
successfully, with nothing in the reply to suggest anything is missing. Walk it
once and the cache holds all 278 thereafter. `wm-atspi` compares each item's
declared child count against the children actually delivered and falls back to
the walk on any shortfall, which is why the `first read` row above says 5.73 s
and not 62 ms: it walked, because the cache asked to be doubted.

**Staying current costs six orders of magnitude less than starting over.** A
drain reads signals that already arrived and asks the application nothing. That
gap — 2 µs against seconds — is the whole reason this project treats polling a
tree as the bug rather than the fallback.

Geometry is a separate row because no bulk API exists for it on either toolkit:
`Cache.GetItems` carries roles, names, states and parentage and no extents at
all, so bounds are a round trip per node even on the fast path.

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
cargo test -p wm-atspi --test live -- --ignored --test-threads=1
```

Without those two variables an SSH session finds an empty desktop and reports
no error worth reading. `wm-probe apps` says what the bus can actually see.

## Licence

Apache-2.0. See `LICENSE`.
