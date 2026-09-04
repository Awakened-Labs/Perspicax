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
| **M0** | Skeleton, conventions, CI | in progress |
| **M1** | Semantic index + AT-SPI ingest — no compositor, runs on X11 | |
| **M2** | Compositor, provenance, occlusion, damage-driven invalidation | |
| **M3** | MCP server and act — **v1** | |

v1 is one demo, run against a GTK app and a Qt app, headless, in CI, with zero
screenshots taken: observe a window, resolve a selector, click it, get a
receipt — and get a *refusal* when the target is occluded.

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

## Licence

Apache-2.0. See `LICENSE`.
