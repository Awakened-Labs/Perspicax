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
perspicax-mcp          MCP server (rmcp, stdio or a Unix socket) — eight tools, DTOs, receipts  [portable]
perspicax-index        node cache, stable ids, selectors, deltas, refusals  [portable]
perspicax-node         node schema — AccessKit types plus Origin and Visibility
perspicax-policy       WM decisions as data — focus, bindings, placement, monitors, workspaces, snapping  [portable]
perspicax-config       config.toml — schema, classic/minimal profiles, feature check  [portable]
perspicax-atspi        impl Ingest — AT-SPI2 over D-Bus
perspicax-compositor   impl HostView — Smithay: outputs, seat, damage. Headless draws nothing.
perspicax-protocols    perspicax's own Wayland protocols — the channel to the desktop shell
perspicax-shell        the desktop — wallpaper, panel, menus, tray, icons; a Wayland client, no Smithay
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

## Roadmap: a window manager a person uses every day

v1 hosts applications for an agent. The W track makes the same compositor one
a person logs in to — KDE-Plasma-like out of the box, and tunable all the way
down to wallpaper and a keyboard. The agent interface is kept working at every
step; it is the reason the compositor exists, not a feature bolted to it.

Two layers of tuning. **Cargo features** decide what is built at all, the way
USE flags do: `seat` (DRM, GBM, EGL/GLES, libinput, libseat), `xwayland`, and
`desktop`, which is both; and one per component of the desktop shell (see
[The desktop](#the-desktop)). **A config file** decides what a build that has a thing does
with it, and a key for something left out of the build is an error naming the
feature, never silently ignored.

| | | |
|---|---|---|
| **W1** | A usable session: DRM from a TTY, libinput, move/resize, keybinds, multi-monitor, clipboard, layer-shell and session-lock (so waybar, fuzzel and swaylock work), Xwayland | done |
| **W2** | Config profiles (`classic`, `minimal`) and policy: focus models, a workspace grid with edge flipping, moving between screens, snapping | done |
| **W3** | Server-side decorations, then tabbed window groups | done |
| **W4** | Protocols Smithay lacks: foreign-toplevel management, ext-workspace, screencopy, output management — and `screenshot` stops refusing; agent verbs to close a window and bring a tab forward | done |
| **W5** | `perspicax-shell`, a separate process: wallpaper, panel, tray, start menu, root menu, desktop icons — each a feature and a toggle | done |
| **W6** | Polish: themes that applications follow, keyboard layouts, XDG autostart, the start menu's ways to leave, a session entry for display managers | done |

The shell is a separate process on purpose. A panel that crashes should not
take every window with it, and anything speaking layer-shell can stand in for
any piece of it.

X11 applications arrive through Xwayland, a client this compositor starts — not
through a second build that is an X11 window manager. There is one display
server path, so there is nothing to keep two builds of in step.

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

**`Window` is not one origin either.** GTK and Qt measure from the window
geometry, the frame a person sees, and report their window node at `0,0`.
Firefox measures from its buffer, client-side shadow included, and reports its
window node at the shadow's width: `26,23` on the first seat it was tried on,
where every agent click on Firefox landed that far from its target while the
receipt said `on_target` (#45). A compositor cannot tell which convention a
toolkit uses, and the toolkit's own window node can. So the index measures
each window from its window node's origin, and refuses every node of a window
whose node reports no extents to measure from.

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

That is an MCP server on stdin and stdout with a compositor behind it.
Headless, it runs until the client goes away, and exits non-zero if the agent
interface failed rather than ended; on `--seat` the session is the person's
and outlives the client. A request sent before `initialize` is answered with
an error naming what to send first, and the server waits on.
`--mcp-socket PATH` serves the same on a Unix socket instead, for an agent
running inside the session it drives: see [An agent inside the
session](#an-agent-inside-the-session). Eight tools: `window_list`, `observe`, `resolve`, `act`, `window_close`,
`tab_forward`, `deltas`, `screenshot`. The two window verbs act on a whole
window by its surface: a close is a request the application may answer with a
dialog, and a tab is brought forward only where the person can already see its
group, never by switching what they are looking at.

`window_list` lists the desk as well as the windows: a panel, a wallpaper or a
menu is `kind: layer`, with the `layer` it stacks in and the
`untrusted_namespace` its program gave it, and a screen locker's cover is
`kind: lock_cover`. An agent can read and click a panel as it would a window;
a window verb aimed at one is refused as `not_a_window`.

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
target's own rectangle, a menu the act opened, lit or closed included. Weigh it
against the idle rates in the table below: an idle GTK window repaints about
forty times a second, so `on_target` means less there than it does on Qt, and
`quiet` — nothing changed at all — is the strong answer on both. A boolean would
have been confidently wrong on one of the two toolkits this project tests
against.

**A refusal is an answer.** It names what is in the way and what would clear it,
so a recoverable situation does not become a retry loop:

```json
{ "kind": "occluded", "message": "node occluded by surface 2", "occluded_by": 2 }
```

`observe` reports the same verdict per node, as `actable` plus `refused`, so an
agent sees an occlusion before it spends a call discovering one. A window on a
workspace that is not showing is refused as `other_workspace`, naming the
workspace, a window that is a tab behind another as `inactive_tab`, naming the
tab in front, and a node hanging off the edge of every monitor as `off_screen`.
None is cleared by the agent switching the person's screen on its own.

A control its own application says is not being shown is refused as
`not_showing`: Firefox's menu bar, hidden until Alt is pressed, and the page of
a tab behind another. A browser draws those into the same surface as the page
in front, over the same pixels, so the compositor's proof that a rectangle is
uncovered is true of the rectangle and not of the node; a click on the hidden
`File` menu would press the tab strip under it. Only the application knows which
of the nodes claiming a rectangle is the one drawn there. Its word is taken
because it can only refuse: an application that says this falsely loses its
own control and nothing else. A disabled control stays actable, since a press
on it lands where the agent aimed and `state.disabled` already says it will do
nothing.

A control its application gives no bounds for is refused as `unplaced`.
Without a rectangle there is nothing to judge, and unlike `unjudged`, waiting
does not help while the application stays silent. A Flutter application is
the case that prompted it: its bridge answers what every control is and never
where, so its labels can be read and none of its controls acted on. A call an
application leaves unanswered costs perspicax a deadline of one second, and a
read stops asking after three, keeping what it has read: one silent call does
not cost the rest of the tree.

A control its application has changed since it was read is refused as
`stale` until it is read again, and perspicax reads it again itself, at once.
If the application does not answer then, or the read stops short of the
control, it reads it again a second later, then two, four, and so on up to a
minute apart, until a read reaches it. A zenity entry stopped for eight
seconds in the middle of a re-read is actable two seconds after it goes on.
An agent has nothing to do but resolve it again in a moment.

`type` has one refusal of its own. Keys go wherever keyboard focus is, so
typing is done only while the node's window holds it, and otherwise refused as
`focus_elsewhere`, naming the surface that does. The agent `focus`es the node
first. The compositor makes that check in the same turn it presses the keys, so
focus cannot move between the two.

While a game holds the pointer ([see Mouse](#a-game-holding-the-pointer)), an
agent's `click` and `scroll`, on the game or anywhere else, are refused as
`pointer_captured`, naming the game as `captured_by`: they would move a pointer
that may not move. So are its `focus` and `tab_forward` on any other window,
which would take the game's keyboard and the pointer with it. Only the person
ends a hold, so waiting for them to pause does not help; the game itself can
still be focused and typed into.

**A titlebar is pixels no client drew.** The frame perspicax draws around a
window goes into the facts beside the window, so a node of another window
under a titlebar is refused as `occluded`, naming the window the titlebar
belongs to, and the verdict is proof rather than policy: the frame is solid
to an agent, even drawn see-through for a person (see [See-through
windows](#see-through-windows)). A window's own frame sits outside it and
never covers its own nodes.
One cost of drawing the frame: a toolkit that would have drawn close and
maximize buttons no longer does, so they are not in its accessibility tree;
`window_close` and `tab_forward` are the agent's way to do what they did.

**Text an application rendered is marked as such.** Every string reaches an
agent under `untrusted_text`, beside the credentials of the process that drew
it — the marking is in the key, so it cannot be skimmed past, and the provenance
is at the point of use rather than in a preamble a model has to have remembered.
That is this project's injection defence, and it is a read-path property rather
than an act-path gate.

**Consent is about the person, not the node.** Headless, there is no capability
gate: perspicax is one actuator among several, an agent refused a click runs the
command instead, and every client is on a socket the agent set up. On `--seat`
the question has two answers, because the person launched most of what is on
screen. There an agent may act only on what perspicax itself spawned
(`Refusal::NoCapability` otherwise), and not at all while the person is using the
keyboard or pointer: an act in the middle of their typing would race it. Typing
only into a window that holds the keyboard is what keeps an agent's keys out of
the person's terminal while it acts on a window it spawned. A window with no
accessibility tree has no node to name, so it cannot be typed into yet
([#38](https://github.com/Awakened-Labs/Perspicax/issues/38)).

**`screenshot` is the fallback, and says so.** Occlusion needs geometry,
z-order, regions and damage, none of which need pixels, so nothing is drawn
for the index and a picture is rendered only when one is asked for: in
software with pixman headless, with the GPU on a seat, behind the `capture`
feature (a build without it answers `not_built`). Every answer carries the
count of nodes under damage no semantic event explained, which is the honest
trigger for a pixel path. A picture comes with an account of every surface in
it and the process that drew it, so no pixel is anonymous, and a window drawn
by a process the agent holds no consent for is painted over in grey and listed
as redacted rather than shown: a picture is a way of reading, and the gate on
reading applies to it.

## An agent inside the session

`--mcp` has one client, whatever started perspicax, and on a seat that is the
login. So the agent a person most wants beside them, one they can see and talk
to in a terminal inside the session, cannot reach it: it would have to start
the very session it is running in. `--mcp-socket` serves the same eight tools
on a Unix socket instead:

```sh
perspicax --seat --mcp-socket "$XDG_RUNTIME_DIR/perspicax-mcp" --spawn foot
```

Every program the session starts is told where the socket is, in
`PERSPICAX_MCP_SOCKET` (empty in a session that serves none), so an agent
started inside it needs only a bridge from its stdio to the socket, and
`perspicax attach` is one. For Claude Code in that terminal:

```sh
claude mcp add perspicax -- perspicax attach
```

It passes the client closing its stdin on as the end of the conversation, and
ends when the session closes the connection. `perspicax attach PATH` names a
socket instead. Anything else that joins stdio to a Unix socket will do, with
OpenBSD netcat's `-N` for the first of those: `nc -N -U "$PERSPICAX_MCP_SOCKET"`,
or `socat STDIO "UNIX-CONNECT:$PERSPICAX_MCP_SOCKET"`.

**One conversation per connection, one at a time.** Another agent's
`initialize` is refused, naming the process that holds the conversation, and
its connection is closed while the first goes on. A conversation ending,
however it ends, ends nothing else: the agent can quit, restart or reconnect,
and the session runs on throughout. Headless too, which runs until it is
killed or `--run-for` ends it. A conversation's `deltas` begin with it, and
what changes while nobody is connected is let go of rather than kept for
nobody. One wrinkle: a client that leaves in the middle of an `act` holds the
conversation for up to five seconds more while the act finishes, and an agent
reconnecting in that time is told the one before still has it.

**Who may connect.** The socket is the owner's alone: mode 0600, and a
connection from a process running as anybody else is closed unanswered. That
is the same user as the programs the agent could drive, but any process
running as you can then drive whatever the agent may -- on a seat, what
`--spawn` started. `$XDG_RUNTIME_DIR`, which only you can enter, is the place
for it. The socket is removed when the session ends. One a killed session left
behind is replaced; one another session is still serving is refused, naming
the process that serves it.

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

Damage is counted across a window's whole surface tree, where in the window it
lands. Firefox draws every page into a subsurface and leaves its toplevel
alone, so a canvas page's frames are its window's, and so are a video player's
or a GTK 4 offloaded picture's. A frame is a commit that brought a new buffer or
named what changed. One that asks for a frame callback and nothing else is not:
loading a page, Firefox committed its subsurface 252 times and drew in 79 of
them.

So is what a window's menus draw. A menu, a combo list or a tooltip is a popup:
a surface of its own, outside the window's tree of subsurfaces. Its accessible
nodes hang off the window's all the same, so its frames are the window's too,
placed where the menu is drawn on it, past the window's edge if it hangs there.
A menu closing is a frame where it was. A panel's menus are the panel's. Each
frame is kept where it landed, not as one box around them all: a menu hanging
below a window and a caret blinking at its top leave every control between
them untouched.

## Build

```sh
cargo build --workspace
cargo test  --workspace
```

The daily-driver build, which needs the development packages for libudev,
libinput, libseat, libgbm, libdrm, libEGL and libGLESv2, and builds the
desktop shell with every component beside it:

```sh
cargo build --release --features perspicax/desktop,perspicax-shell/full
```

perspicax starts `perspicax-shell` from beside its own binary, and from `PATH`
if there is none there. A workspace build without `perspicax-shell/full`
leaves a shell of no components in `target/`, which draws nothing; to install
the shell on its own, `cargo install --path crates/perspicax-shell --features
full`.

Run it from a text console (a TTY, not a terminal inside another desktop), with
seatd or logind managing the seat. Log to a file: the console the session
takes over shows nothing until it ends. With no `RUST_LOG`, a seat logs its
warnings; `RUST_LOG=info` says what it is doing as well.

```sh
perspicax --seat --spawn foot 2>~/perspicax.log
```

Two chords always work, whatever the configuration says:
**Ctrl+Alt+Backspace** ends the session, and **Ctrl+Alt+F1…F12** switches VT.

X11 applications run under an Xwayland the session starts (`xwayland = false`
in the config turns it off), with `DISPLAY` set for everything it launches:
`--spawn` programs and the autostart list start once Xwayland is ready.
Toolkits are asked for Wayland first, and allowed X11 after it (GTK is told
`wayland,x11`, Qt `wayland;xcb`), so a program that will not use Wayland --
Chromium or Electron in X11 mode, say -- opens on this Xwayland rather than
failing to open a display. An
X11 window's origin says so: the X client's pid comes from the X server's
X-Resource answer rather than the kernel, and every X client shares one consent
decision, because X11 lets them read and drive each other.

An X11 window gets what a Wayland one gets, and is told so in X11's terms. One
that asks to go fullscreen -- a game, as Wine asks -- covers its monitor and
the panels, with no frame, and keeps it if it then asks for another size; one
that asks to be maximized fills what the panels leave. Either may ask before
it even maps, as a game that starts fullscreen does. Minimized, by its own
request or from the taskbar, an X11 window is told it is iconic (ICCCM's
`WM_STATE`, and `_NET_WM_STATE_HIDDEN`), and normal again when it comes back,
which is the change Wine waits for before it draws again. One that maps
asking to start minimized (ICCCM's initial state, as `xterm -iconic` and Wine
set it) starts that way and is told so, and the keyboard stays where it was.
A window on a workspace that is not showing is not minimized, and is not told
it is. Every request is answered, even one that changes nothing, because Wine
changes nothing more about a window while one of its requests is waiting. As
with a Wayland window, all of this is for a person at the seat: an agent's
headless desk does not rearrange itself.

An X11 window may also ask to be the active one (`_NET_ACTIVE_WINDOW`), as a
program does when a second copy of it is started, or as Wine does to bring a
window back. It comes back from minimized and is raised, but takes the
keyboard only with fresh input behind the request, as a Wayland window needs
an activation token for: the X time of that input, under ten seconds old.
Wine sends none, so its window comes back without the keyboard; without
input a window on a workspace that is not showing waits there, since showing
that workspace would move the keyboard. Asking with `_NET_WM_STATE_HIDDEN`
minimizes or brings back, as Openbox takes it. And the root's
`_NET_ACTIVE_WINDOW` names the X11 window with the keyboard, or none while a
Wayland window has it, which Wine goes by for the window in front.

The session reads `$XDG_CONFIG_HOME/perspicax/config.toml` (or `--config PATH`),
and reads it again whenever it is saved. Without one it runs the `classic`
profile, which is Plasma's and Windows' habits:

- click to focus, Alt+F4, Alt+Tab;
- Alt+drag to move and Alt+right-drag to resize;
- four workspaces in a row: Ctrl+Logo+arrows switches, add Shift to take the
  focused window along, Ctrl+F1…F4 goes straight to one;
- dragging a window to an edge of the desk snaps it to half the monitor, a
  corner to a quarter, the top to maximized, with Logo+arrows to do the same
  from the keyboard;
- Logo+Shift+Left/Right moves a window between screens, Logo+Shift+R reloads;
- a tap of Logo on its own asks the desktop shell for its start menu;
- a titlebar drawn by the compositor for a client that asks for one (Qt, foot,
  GTK 3 without a headerbar, and X11 applications): drag it to move,
  double-click it to maximize, and minimize, maximize and close at its right;
  drag an edge or a corner, or just outside one, to resize;
- tabs, as Fluxbox has them: drag a titlebar with the middle button onto
  another window's to make the two tabs of one window, and a tab away to part
  them. Logo+G does the same from the keyboard with the window focused before,
  Logo+Tab steps through the tabs, and Logo+Shift+G takes one out. A window
  that draws its own frame gets a strip of tabs above it while it is grouped.

`minimal` is Fluxbox's and Enlightenment's: focus follows the pointer, a 2×2
grid of workspaces that wraps, and resting the pointer against an edge of the
desk flips to the next one, taking along a window being dragged, as does
scrolling over the desktop. A window being moved stops at the edge of a
screen or a panel until it is pushed 20 pixels further. The pointer itself
stops at the edge of the desk, so a window grabbed nearer than that to the
side it is pushed towards, as a titlebar is to the top, stays on screen.
It does not snap. In both, a fullscreen window
covers the panels while it is the one in use, and goes back under them when
another window, the start menu or a launcher takes the keyboard. A menu of
its own, a video's right-click menu, leaves it over them, while it is open
and as it closes. While it covers them, its edges
are its own: resting the pointer against one does not flip, so a game turning
its camera stays where it is. So are a window's that holds the pointer,
fullscreen or not ([see Mouse](#a-game-holding-the-pointer)). Nor does an edge
flip behind the lock screen.
Every key below is optional and overrides the profile one setting at a time.
A misspelled key, or a key for a
feature this build left out, is refused with its name rather than ignored.

```toml
profile = "minimal"            # or "classic"; minimal focuses under the pointer
drag = "Logo"                  # the drag modifier; "none" turns drags off
autostart = [["mako"], ["nm-applet", "--indicator"]]
xdg-autostart = true           # and the autostart folders' entries; see below
                               # (every key outside a table goes up here)

[focus]
model = "sloppy"               # click | sloppy | strict
autoraise = false

[keys]
"Logo+Return" = { spawn = ["foot"] }
"Logo+d" = { spawn = ["fuzzel"] }
"Alt+F4" = "none"              # hand a profile's chord back to the client
"Alt+F1" = "root-menu"         # or "start-menu": the desktop shell's menus;
                               # { pie = "<name>" } opens a pie, see Pies
"Logo" = "none"                # Logo alone is a tap: pressed, let go, nothing
                               # in between; no other modifier can be tapped

[mouse.desktop]                # buttons and the wheel, by where the pointer is;
"Mouse8" = "workspace-left"    # see Mouse below
"Mouse9" = "workspace-right"

[input.keyboard]
layout = "gb"
options = "ctrl:nocaps"
repeat-rate = 30

[input.pointer]
natural-scroll = true
tap-to-click = true
double-click-ms = 400          # titlebars and the shell's desktop icons alike

[workspaces]
mode = "spanning"              # one workspace across every monitor, switched
                               # together; "per-output" flips each on its own
grid = [3, 2]                  # columns, rows
wrap = true

[workspaces.flip]
edge = true                    # rest the pointer on an outer edge of the desk
delay-ms = 300
while-dragging = true          # and take the window being dragged along
scroll = true                  # scroll over the desktop: down and right to the
                               # next one; see Mouse to turn it round

[snap]
drag = true
threshold = 4                  # pixels from the edge

[resistance]
edges = 20                     # a window being moved stops at the edge of a
                               # screen or a panel until pushed this many
                               # pixels past it; 0 lets it straight through
seams = 0                      # the same where two monitors meet

[opacity]                      # windows drawn see-through, for your eyes
unfocused = 85                 # only; see See-through windows below

[decorations]
mode = "server"                # draw titlebars for clients that ask; "client"
                               # tells every client to draw its own
title-height = 24
border = 2
focused = "#2d6fa3"            # the title is written in black or white,
unfocused = "#475057"          # whichever reads on the colour

[[output]]
name = "DP-1"
mode = "2560x1440@144"

[[output]]
name = "HDMI-A-1"
right-of = "DP-1"              # or left-of, above, below; or position = [x, y]
offset = 180                   # along the shared edge: lower a smaller monitor
scale = 1.25

[[output]]
name = "eDP-1"
enable = false

[protocols]                    # who may reach past their own windows:
foreign-toplevel-management = "any"   # "any", "off", or a list of programs
workspace = "any"
screencopy = ["grim", "/usr/bin/wf-recorder"]   # a name, or a full path
output-management = ["kanshi", "wlr-randr"]
shell = ["/usr/local/bin/perspicax-shell"]
```

`[protocols]` names the programs that may use the protocols reaching past
their own windows: a taskbar's list of windows (`foreign-toplevel-list`, and
`foreign-toplevel-management` to activate and close them), a pager
(`workspace`), a screenshot tool (`screencopy`), a display tool
(`output-management`) and the desktop shell (`shell`), which is told when a
key asks for a menu or a pie and may end the session. Listing windows and workspaces
is open by default; reading pixels, moving monitors and speaking for the shell
are for the usual programs, by name. A name is
whatever the kernel says the client is running, which any program can be
called, so a full path is the stricter form. Whatever this says, all of them
are inert while the screen is locked.

| Protocol | `[protocols]` key | Spoken by | While locked | When the rule narrows |
|---|---|---|---|---|
| `ext-foreign-toplevel-list-v1` | `foreign-toplevel-list` | window lists | nothing new is told; told on unlock | every window closed, list finished |
| `wlr-foreign-toplevel-management-unstable-v1` v3 | `foreign-toplevel-management` | waybar `wlr/taskbar` | requests ignored | every handle closed, manager finished |
| `ext-workspace-v1` | `workspace` | waybar `ext/workspaces` | switches ignored | everything removed, manager finished |
| `wlr-screencopy-unstable-v1` v3 (shm) | `screencopy` | grim, wf-recorder, xdg-desktop-portal-wlr | every copy fails | waiting frames fail |
| `wlr-output-management-unstable-v1` v4 | `output-management` | wlr-randr, kanshi, wdisplays | every configuration fails | manager finished |
| `perspicax-shell-v1` (perspicax's own) | `shell` | perspicax-shell | told nothing; log-out ignored | finished |

A rule a reload changes applies to the next client that looks, with no
global torn down, and what a client already holds is withdrawn as above.
`screencopy` needs the `capture` feature (part of `desktop`). A tool that asks
for the pointer (`grim -c`, OBS's *Show cursor*) gets it drawn in, and a
recording waiting for a change is sent one when the pointer moves. A display tool's
change to the monitors lasts for the session: it is put in force as the output
rules, exactly as if `[[output]]` had said it, and a reload that changes
`[[output]]` puts the file back in charge. Moving a monitor just moves it; a
new mode, scale, or a monitor turned on or off lights the monitors again.

Monitors are placed relative to each other, so the layout survives one being
unplugged: a monitor beside one that is missing goes to the right of the
rest, and windows left on a monitor that goes away come onto the nearest one
that remains. Only the outer edges of the desk flip and snap; between two
monitors the pointer passes through, and so does a window being moved unless
`[resistance] seams` holds it there.

None of that needs a second monitor to try. `--headless --size 2560x1440
--size 1920x1080` runs two virtual ones, which is what the live tests in
`crates/perspicax-compositor/tests/` do. On a seat, the kernel will light a
connector with nothing plugged into it on request (`echo on | sudo tee
/sys/class/drm/card1-HDMI-A-1/status`, and `detect` to undo it), and
perspicax treats it as a monitor nobody can see: the pointer crosses into it,
windows can be sent there and back, and unplugging it rescues them.

## Log in to it

A display manager (SDDM, GDM, LightDM, greetd) can offer Perspicax at its
greeter beside every other session. Build as yourself, as above, and install
as root:

```sh
cargo build --release --features perspicax/desktop,perspicax-shell/full
sudo scripts/install.sh
```

That puts `perspicax` and `perspicax-shell` side by side in `/usr/local/bin`,
the session entry in `/usr/local/share/wayland-sessions`, and the settings
portal's two files under `/usr/local/share/xdg-desktop-portal`. Some programs
read only from `/usr`: LightDM, greetd's greeters and older SDDM look for
sessions only in `/usr/share/wayland-sessions`, and xdg-desktop-portal for
portal files only in `/usr/share/xdg-desktop-portal/portals`. The script says
when either applies; `sudo PREFIX=/usr scripts/install.sh` installs under
`/usr`, and `PORTALDIR` moves the portal file alone. It builds nothing, and
`DESTDIR` and the other variables at its top are for a package.

Choose Perspicax at the greeter. The entry runs `perspicax --session`, which is
`--seat` with two differences, for a session nobody watches start:

- **A bus of its own.** A display manager that is not systemd's starts a
  session with no D-Bus session bus, and the tray, the keyring and the portals
  each need one. With no `DBUS_SESSION_BUS_ADDRESS`, and nothing answering at
  `$XDG_RUNTIME_DIR/bus`, it runs itself again under `dbus-run-session`. That
  bus ends with the session, and every service it started ends with it. The
  session stops the accessibility bus itself on the way out, since that one,
  left to the display manager's hangup, would outlive it.
- **A log of its own**, at `~/.local/state/perspicax/perspicax.log` (under
  `$XDG_STATE_HOME` when that is set), readable by you alone. The last
  session's is kept as `perspicax.log.old`: that is the one to read after a
  login that went straight back to the greeter. It says what the session did
  as well as its warnings; `RUST_LOG` changes that, as ever.

For a keyring the login unlocks, the two `pam_gnome_keyring` lines under "The
desktop" go in the display manager's PAM file: `/etc/pam.d/sddm` for SDDM.
Applications follow the theme's dark or light through the settings portal:
xdg-desktop-portal finds perspicax's backend from the portal file and
`XDG_CURRENT_DESKTOP=perspicax`, which the entry sets. `perspicax-portals.conf`
sends the rest to GTK's backend (the file chooser, printing) and screen sharing
to wlroots', so install xdg-desktop-portal-gtk and -wlr for those. To see
which backends it chose, run it again from a terminal inside the session,
`/usr/libexec/xdg-desktop-portal -rv` (`/usr/lib/` on some distributions).

**What starts with a session**, from a greeter or a text console alike: the
desktop shell, then the config's `autostart` list, then the entries in the
XDG autostart folders, `~/.config/autostart` and `/etc/xdg/autostart`, each
once. An entry of yours replaces the system's of the same file name, and one
of yours saying `Hidden=true` turns the system's off. An entry is skipped when
it is hidden or disabled (`X-GNOME-Autostart-enabled=false`), when its
`OnlyShowIn` or `NotShowIn` rules out `perspicax`, or when its `TryExec`
program is not installed; one that needs a terminal is skipped with a line in
the log, and `RUST_LOG=info,perspicax_compositor=debug` names every entry
skipped and why. `xdg-autostart = false`, at the top of the config, turns the
folders off and keeps the list. A program both of them name starts twice:
take it out of one.

Another desktop on the same machine may never have started what those
folders hold -- Enlightenment starts only what its own startup list names --
so a first login here can start something that has never run there before:
PipeWire beside a PulseAudio that was the sound server until then, for one.
Turn off what you do not want with a `Hidden=true` entry of your own.

## The desktop

`perspicax-shell` is the desktop: a wallpaper, a panel, menus of the installed
applications, other programs' tray icons and the desktop folder's icons. It
is a program of its own and an ordinary Wayland client: its surfaces are
layer-shell surfaces, and its taskbar and pager speak the protocols waybar
speaks. A panel that crashes takes no window with it, and anything speaking
layer-shell can stand in for any piece of it.

perspicax starts it as the session starts, before `autostart`, and starts it
again if it crashes; a shell that refuses its config waits for the file to be
saved again. A save that changes `[shell]` reaches it in place, without a
restart. The tray needs a D-Bus session bus, and a text-console login has
none: start the session under one, `dbus-run-session -- perspicax --seat`, or
as `perspicax --session`, which starts one itself.

The session tells its bus what it is and where its displays are, so that a
program D-Bus starts on request -- a keyring's unlock prompt, a notification
daemon, a portal -- has somewhere to draw: `XDG_CURRENT_DESKTOP=perspicax`,
`XDG_SESSION_TYPE=wayland`, `WAYLAND_DISPLAY` and, once Xwayland is up,
`DISPLAY`, which a systemd user manager is told too when there is one. It also
asks the bus for the Secret Service as it starts, and says in the log when
that does not come up. For a keyring your login unlocks, the PAM file for how
you log in (`/etc/pam.d/login` from a text console, `/etc/pam.d/sddm` through
SDDM) needs `-auth optional pam_gnome_keyring.so` and `-session optional
pam_gnome_keyring.so auto_start`; without them, the first program that wants
a secret asks for the keyring's password. gnome-keyring serves one session's
bus at a time, so a second session of yours, beside another desktop that is
still logged in, has none: every lookup there waits 25 seconds and fails, and
the log names the keyring that is in the way. The desktop portal waits the same
25 seconds as it starts, and every GTK 4 application uses the portal as it
opens, so the session starts the portal itself a few seconds in rather than
leave that wait to the first application; one opened sooner waits for what is
left of it.

Each component is a cargo feature of `perspicax-shell`, and `full` is all of
them; none is on by default. The profiles turn on what the build has:

| Component | Feature | `classic` | `minimal` | Surface: layer, namespace | What an agent reads |
|---|---|---|---|---|---|
| Wallpaper | `wallpaper` | Plasma's blue | a dark grey | background, `perspicax-desktop-<output>` | a `Window` named for its surface |
| Desktop icons | `icons` (with `wallpaper`, `menus`) | the first monitor | off | drawn on the wallpaper's | a `List` "Desktop" of a `ListItem` for each icon, the selected one selected |
| Root menu | `menus` | right-click on the wallpaper, or a key or button bound to `root-menu` | the same | overlay, `perspicax-menu-<output>`, while open | a `Menu` "Root menu" of `MenuItem`s |
| Panel | `panel` | along the bottom of every monitor | none | top, `perspicax-panel-<output>` | a `Toolbar` "Panel" holding what follows |
| Start button and menu | `menus` | the panel's first item; a tap of Logo | — | the panel's; its menu as the root menu's | a `Button` "Start"; a `Menu` "Start menu", with a search line unless `search = "none"` |
| Taskbar | `panel` | each monitor's own windows | — | the panel's | a `TabList` "Taskbar", the window in use selected |
| Pager | `panel` | the workspaces | — | the panel's | a `TabList` "Workspaces", the one showing selected |
| Keyboard layout | `panel` | before the tray, with two layouts or more | — | the panel's | a `Button` "Keyboard layout", the layout's name its value |
| Tray | `tray` (with `panel`, `menus`) | before the clock | — | the panel's; its menus as the root menu's | a `Group` "Tray" of a `Button` for each icon, named by its tooltip |
| Clock | `panel` | the time, last | — | the panel's | a `Status` "Clock", the time its value |
| Pies | `pie` (with `menus`, `panel`) | a key or button bound to `{ pie = "<name>" }` | the same | overlay, `perspicax-pie-<output>`, while open | a `Menu` named for the pie of a `MenuItem` for each slot, the one pointed at selected |

An agent reads the shell as it reads any application. On a seat it cannot
click it: like everything else the person's session starts, the shell gets no
agent consent. A key for a component the shell was built without is refused
with the feature that would provide it. Every key is optional:

```toml
[shell]
enabled = true                 # false: no shell; bring your own from autostart
wallpaper = "~/Pictures/wall.jpg"   # an image, "#rrggbb", or "none"; a
                               # relative path is beside this file
wallpaper-mode = "fill"        # fill | fit | center | tile
root-menu = true
menu-file = "menu.toml"        # extends the root menu, or replaces it
desktop-icons = true
untrusted-launchers = "hidden" # an application's entry on the desktop that is
                               # not executable: "as-files" (the default) shows
                               # it as the file it is
icon-theme = "Adwaita"
terminal = ["foot"]            # for applications that ask for one
lock = ["swaylock", "-f"]      # the start menu's Lock,
suspend = ["loginctl", "suspend"]   # Suspend, Restart and Shut Down:
reboot = ["loginctl", "reboot"]     # logind's, which elogind has too
power-off = ["loginctl", "poweroff"]
leave = ["lock", "log-out", "suspend", "reboot", "power-off"]   # and which
                               # of them the start menu shows, in this order

[shell.wallpapers]             # a workspace's own, by number from 1, row by row
2 = "#2d5a4f"
3 = { wallpaper = "~/Pictures/tile.png", mode = "tile" }

[shell.panel]
enabled = true                 # false: no panel
edge = "top"                   # or "bottom"
height = 32
width = 600                    # or "60%" of the monitor; "100%" by default.
                               # Windows keep out of the whole strip all the
                               # same. A bar too narrow for what it holds
                               # shrinks its tasks to their icons, then widens
align = "center"               # "left" or "right" of the strip: only with a
                               # width
outputs = "first"              # "all", or a list of connectors: ["DP-1"]
taskbar = "all"                # every window on every panel; "this-output"
                               # lists each monitor's own
task-titles = false            # each task its icon alone; true, by default,
                               # writes its window's title beside it
items = ["start", "taskbar", "pager", "layout", "tray", "clock"]
clock = "%a %e %b %H:%M"       # as strftime writes it
```

The menu file is TOML too: `mode = "extend"` puts its items above the
applications and `"replace"` puts them instead, and each `[[items]]` is one of
`exec = [...]` with a `label`, `app = "firefox"` for an installed
application, `separator = true`, a submenu with `items = [...]`,
`applications = true` for the applications by group, or `session = true` for
the ways to leave.

The start menu is written in the same words, in `config.toml` itself, under
`[shell.start-menu]`; the menu file is the same items in a file of its own.
With no table, the start menu is every application by group, then the ways to
leave, and its button is Perspicax's mark. A few chosen entries, then the ways
to leave:

```toml
[shell.start-menu]
icon = "start-here"            # the start button's: an icon theme's name, or
                               # an image file, anything with a "/" in it
                               # ("~/Pictures/logo.png", or "./logo.svg" beside
                               # this file); unset, or not found, is
                               # Perspicax's mark
mode = "replace"               # "extend" (the default) puts the items above
                               # the applications; "replace" puts them instead
search = "all"                 # what typing finds: "menu" (the default), what
                               # the menu holds; "all", every application too;
                               # "none", and there is no search line
items = [
  { app = "firefox" },
  { app = "org.kde.dolphin", label = "Files" },
  { label = "Terminal", exec = ["foot"], icon = "utilities-terminal" },
  { label = "Projects", items = [
      { label = "perspicax", exec = ["foot", "-D", "~/code/perspicax"] },
  ] },
  { separator = true },
  { session = true },          # Lock, Log Out, …, as [shell] leave says
]
```

Nothing but the ways to leave:

```toml
[shell.start-menu]
mode = "replace"
search = "none"
items = [{ session = true }]
```

A saved change shows the next time the menu opens, and a new icon at once. An
`app` that is not installed is left out rather than refused, so one config can
travel between machines; a `"replace"` menu with no items is refused, since it
would never open. The button is a `Button` "Start" to an agent whatever it
wears.

The start menu ends with those ways to leave, unless `[shell.start-menu]`
puts its items in their place without `session = true`: Lock, Log Out, Suspend, Restart
and Shut Down, as `leave` chooses and orders them (`leave = []` shows none).
Each runs its program, and shows only when that program is on `PATH`; Log Out
asks the compositor instead. Typing "sleep", "reboot" or "power off" in the
search line finds the last three. Suspend does not lock the screen first;
`["swayidle", "-w", "before-sleep", "swaylock -f"]` in `autostart` does.

A tray icon is any program's that registers one the StatusNotifierItem way,
as Qt, Electron and libappindicator programs do. The shell serves the
registry the programs look for, or shows what another program's lists if one
was there first. A left click activates the icon, a middle click activates it
the other way, and a right click opens its menu at the icon, as the start menu
opens at its button.

### Pies

A pie is PieDock's: a ring of icons that opens centred on the pointer when a
key or a button bound to `{ pie = "<name>" }` is pressed, and chooses by
direction. Pointing anywhere past its middle, out to the screen's edge,
picks the icon that way, which grows as the pointer turns toward it and has
its name in the middle. Then:

- **Left button.** It opens a submenu in the pie's place, or starts the
  program. If the program has windows open, it brings the next of them
  forward instead, so pressing again goes round them.
- **Middle button.** It starts the program, whatever is open.
- **Right button.** It goes back out of a submenu, and closes the pie from
  its first ring.
- **Wheel.** It spins the pie a place a notch.
- **Keys.** Escape closes it, Enter is the left button, Up and Down spin
  it, and Backspace is the right button.
- **Its own binding.** Pressed again, it closes the pie.

```toml
[mouse.anywhere]
"Mouse8" = { pie = "launchers" }    # the thumb button, taken from every window

[shell.pie]
size = 512                          # across, in pixels; 128 to 1024
icons = "~/.piedock/icons"          # <name>.png or .svg, any case, looked for
                                    # before the icon theme
aliases = [                         # what names a running window: its app-id,
  { app-id = "com.mitchellh.ghostty", as = "ghostty" },   # unless one of these
  { title = "Steam", as = "steam" },                     # matches it exactly
]
ignore = [{ title = "Picture-in-Picture" }]   # windows no pie shows

[shell.pie.menus]
launchers = [
  { label = "terminal", exec = ["foot"] },  # its icon is its label's, unless
                                            # `icon` names another
  { app = "firefox" },                      # an installed application, or a
  { app = "~/apps/tool.desktop" },          # desktop file by its path
  { label = "games", items = [              # a submenu
    { label = "steam", exec = ["steam"] },
  ] },
  { running = true },                       # each running application no slot
]                                           # of this ring stands for
```

A pie is written in the menu file's words, less `separator`, `applications`
and `session`, plus `running`, which only a first ring has, once. A running
window is known by the first alias that matches its app-id or title, and
otherwise by its app-id. An X11 window's app-id is its `WM_CLASS` class. That
name finds the slot that stands for it (a program's label, or an application
whose desktop entry claims the window) and its icon in the folder. A slot
with windows open has a dot for each, up to three.

**Coming from W4.** `classic` now starts the shell, so a config that starts
waybar and swaybg from `autostart` gets two panels and two wallpapers. Take
them out of `autostart`, or keep them and set `[shell] enabled = false`.

## Themes

One `[theme]` table colours everything perspicax draws -- the titlebars, the
panel, the menus, the desktop's labels -- and tells applications whether to be
dark or light. Every key is optional, and a save reaches all of it in place.

```toml
[theme]
name = "breeze-dark"           # perspicax (the default), breeze-light, breeze-dark
font = "Noto Sans"             # the shell's and the titles'; sans-serif (the
                               # default), serif, monospace, or a family's name
font-size = 15                 # the shell's text: 10 to 20 pixels, 14 unset
cursor = "breeze_cursors"      # an Xcursor theme; unset, XCURSOR_THEME's
cursor-size = 32               # 8 to 128; unset, XCURSOR_SIZE's
color-scheme = "dark"          # what applications are told: dark, light or
contrast = "high"              # no-preference; normal or high

[theme.palette]                # any role below, written over the theme's own
accent = "#e93d5a"
panel = "#102030"
```

`perspicax` is what was drawn before there were themes: blue titlebars, a dark
panel, light menus. The two Breezes take their window, view, header and
selection colours from Plasma's Breeze schemes.

The palette is a set of roles, not of widgets, so one colour written recolours
everything that has that job. A colour is `"#rrggbb"`. Only `selected` and
`label-shadow` may be see-through, as `"#rrggbbaa"`: an opaque panel or menu
is what lets the compositor prove to an agent what it covers. Writing one
colour does not mean writing six. A role you leave out follows from the ones
you wrote, as the third column says, and otherwise keeps the theme's own. An
ink is black or white, whichever reads on its colour; a mix is that far from
the first colour toward the second. The last column is the `perspicax` theme.

| Role | Colours | Unwritten, follows | `perspicax` |
|---|---|---|---|
| `accent` | whatever is chosen or in use: the menu line under the keyboard, the task in use, the workspace showing | — | `#3daee9` |
| `on-accent` | text written on the accent | the accent's ink | `#ffffff` |
| `title-focused` | the titlebar and border of the window with the keyboard | — | `#2d6fa3` |
| `title-focused-ink` | its title and buttons | its ink | `#ffffff` |
| `title-unfocused` | every other window's titlebar and border | — | `#475057` |
| `title-unfocused-ink` | their titles and buttons | its ink | `#ffffff` |
| `snap-preview` | where a window dragged to an edge would snap, a quarter opaque | — | `#8cb3f2` |
| `panel` | the panel's background | — | `#232629` |
| `panel-ink` | text and symbols on the panel | its ink | `#fcfcfc` |
| `panel-rule` | the rule along the panel's edge, and the edge of a workspace not showing | `panel` 12% toward `panel-ink` | `#3b4045` |
| `panel-face` | a task's button, and a workspace not showing | `panel` 7.5% toward `panel-ink` | `#31363b` |
| `panel-open` | the start button with its menu open, the task in use, the workspace showing | `panel` 30% toward `accent` | `#2b4f63` |
| `panel-faint` | a minimized task's title, and the window drawn for one with no icon | `panel` 57% toward `panel-ink` | `#9aa0a6` |
| `menu` | a menu's background | — | `#fcfcfc` |
| `menu-ink` | a menu's text | its ink | `#232629` |
| `menu-edge` | a menu's border | `menu` 42% toward `menu-ink` | `#a0a4a8` |
| `menu-rule` | the line between groups of items | `menu` 15% toward `menu-ink` | `#dcdee0` |
| `menu-opened` | the line whose submenu is open | `menu` 30% toward `accent` | `#c4e5f7` |
| `menu-typed` | the start menu's search line | `menu` 6% toward `menu-ink` | `#eff0f1` |
| `menu-hint` | the search line's hint, and an item that cannot be chosen | `menu` 55% toward `menu-ink` | `#7f8c8d` |
| `selected` | the desktop icon chosen, over the wallpaper | `accent`, 40% opaque | `#3daee966` |
| `label-ink` | a desktop icon's name | — | `#ffffff` |
| `label-shadow` | the shadow that name casts, so it reads on any wallpaper | — | `#000000c0` |

`[decorations] focused` and `unfocused` are `title-focused` and
`title-unfocused` written in their older place, and still win over any theme;
writing one in both places is refused.

`font` is the family of the shell's text and of the titles. `font-size` sizes
the shell's text, and the desktop labels' lines with it; menu rows keep their
height, and a title's size follows its bar's. A family with no face installed
falls back to the system's, with one warning in the log. The pointer is drawn
from `cursor` at `cursor-size`; programs started afterwards are told both
(`XCURSOR_THEME`, `XCURSOR_SIZE`), and those that ask for a cursor by its
shape get the compositor's.

Applications are told through the settings portal (see [Log in to
it](#log-in-to-it)): `color-scheme`, `contrast`, and the accent, which is the
palette's `accent` when written and Breeze's blue under either Breeze. The
`perspicax` theme tells them nothing, so they look as they would with no
portal at all. GTK 4 follows at once, a change while it runs included, and
libadwaita takes the accent from GNOME 47 on, rounded to the nearest of its
nine. Firefox follows on "System theme — auto". GTK 3 and Qt 5 ignore the
portal's colour scheme, and Qt 6 follows only with a platform theme that
reads it.

## See-through windows

A window can be drawn see-through: by its application, by a key, and dimmed
while it does not have the keyboard. All of it is off until written.

```toml
[keys]
"Logo+Page_Up" = "opacity-up"       # the focused window, a step at a time
"Logo+Page_Down" = "opacity-down"
"Logo+End" = "opacity-reset"        # back to its application's, or opaque

[opacity]
step = 10              # percent a press moves it; the default
floor = 20             # the lowest the keys take a window; the default
unfocused = 85         # a window without the keyboard, as a share of its
                       # own; 100, the default, dims nothing

[opacity.apps]         # where each application's windows start
foot = 90
"org.gnome.Nautilus" = 95
```

Every value is a whole percent from 1 to 100, and `unfocused` and each
application's may not be below `floor`. No profile binds the three actions;
any key or mouse binding can.

- **A window starts at its application's value,** or opaque. The app id
  under Wayland, or the `WM_CLASS` class under X11, is matched exactly, case
  included.
- **The keys move it from there,** within `floor` and opaque. What they set
  stays with that window, through reloads, until `opacity-reset`; a reload
  changes every other window at once.
- **A window without the keyboard** is drawn at `unfocused` of its own: foot
  at 90 with `unfocused = 85` shows at about 77. That can go below `floor`,
  which is only how far the keys go. A window whose own menu is open, or
  has just closed, still has the keyboard: its application is still told
  it is the active window, and its task stays lit. While the start menu or
  a launcher holds it, no window does, and every one is dimmed, as every
  titlebar shows unfocused.
- **A fullscreen window is drawn opaque,** so a video is never dimmed, and
  the keys leave it alone. Its own returns when it leaves fullscreen.
- **A tab group shares** what the keys set; with nothing set, each tab
  follows its own application.
- **The whole window shows through:** its titlebar and border, and its
  menus. Each of its surfaces is drawn on its own, so where an application
  draws one over another, as a browser does a video, the one beneath shows
  faintly through it.
- **X11 menus and tooltips are drawn opaque,** since nothing ties one to its
  window. A client's own `_NET_WM_WINDOW_OPACITY` is ignored.

**For your eyes, not an agent's.** A see-through window still covers what is
behind it as far as an agent is told: a node beneath it is `occluded`, naming
it, exactly as if it were opaque. You chose to look through it, and an agent
is not expected to read what shows there. A picture of the monitor shows what
you see, the text faintly behind the window included, and `window_list` gives
such a window an `opacity`, so an agent shown that text and told it is
covered has the reason. A picture of the window by itself shows it whole.

## Keyboards

`[input.keyboard]` takes xkb's names for a keymap, as every other desktop does,
with up to four layouts between which a key switches -- for the whole session,
or for each window on its own.

```toml
[input.keyboard]
layout = "us,ru"               # with variant, options, model and rules
variant = ",phonetic"
options = "ctrl:nocaps"
numlock = true                 # on from the start; unset leaves Num Lock alone
switching = "window"           # each window keeps its own layout; "global",
                               # the default, switches the session's

[keys]
"Logo+Space" = "next-layout"   # classic's own; also previous-layout, and
                               # layout-1 to layout-4 for one in particular
```

xkb's own switching options, `grp:alt_shift_toggle` and the like, work as
well. classic binds Logo+Space, so applications no longer get it. A reload
keeps the layout in use, and Caps Lock and Num Lock with it. The lock keys
light their LEDs on every keyboard, one plugged in later included.

Under `switching = "window"`, a window is typed in the layout it was last typed
in, and a new one starts in the first. The panel, the start menu or the lock
screen taking the keyboard changes nothing. A window's own menu is typed as
the window, so a layout switched while it is open stays that window's.

With two layouts or more, the panel's `layout` item shows the one in use, its
name in capitals (`US`, `RU`), and a click moves to the next. An agent reads it
as a `Button` "Keyboard layout" whose value is the layout's full name. An
agent's `type` writes in the layout in use when that has the character, and
otherwise switches to the first layout that has it, types, and switches back,
with Caps Lock respected; only a character no layout in the keymap can make is
refused.

Switching is the one binding that works at the lock screen, so a password can
be typed in the layout it was set in. Hold Logo while Space goes down: swaylock
lights its ring on a bare Logo, which invites a pause, and a Space pressed after
Logo is let go types a space into the password.

A chord names a key by its character, and matches first what the key makes in
the layout in use. Two fallbacks keep the usual chords where hands expect them
away from a US keyboard:

- Under a layout with no Latin letters, a letter chord matches the key's letter
  in the first layout that has one: under `"us,ru"`, Logo+q is the same key in
  either. It needs a Latin layout in the list; `"ru"` alone has none to borrow.
- A key whose shifted character is a digit counts as that digit, so AZERTY's
  Logo+1 is the key marked 1, not Logo+&.

Two cases are not solved. A chord on punctuation matches the key that makes
that character in the layout in use, which on many layouts needs Shift or
AltGr; a chord's modifiers must match exactly, so such a chord cannot be
pressed as written. And a dead key, an accent waiting for its letter, makes no
character to write a chord with.

## Mouse

`[mouse]` binds a mouse's buttons and its wheel to the actions `[keys]` takes,
in four tables, by where the pointer is:

```toml
[mouse.desktop]                # the empty desktop: no window, no panel
"Mouse8" = "previous-workspace"
"Mouse9" = "next-workspace"    # both in reading order, as scroll flipping goes
"Mouse2" = "root-menu"

[mouse.titlebar]               # a window's title, or its tabs
"Double+Mouse1" = "minimize"   # instead of maximizing
"Mouse3" = { spawn = ["foot"] }

[mouse.window]                 # anywhere on a window, its frame or what it drew
"Logo+Mouse9" = "toggle-sticky"

[mouse.anywhere]               # wherever no other table says
"Logo+Mouse8" = "workspace-left"
```

A press is looked for in the titlebar's table, then the window's, then
`anywhere`; over the desktop, in the desktop's, then `anywhere`. Over a panel
or a menu only `anywhere` applies. A binding comes before everything the
compositor does with a press itself -- a titlebar dragged or double-clicked, a
tab picked up with the middle button, the drag modifier, scroll flipping -- so
`"Double+Mouse1"` above replaces the maximize, and neither the press nor its
release reaches an application. `"none"` hands a button back where it is
written: `[mouse.window] "Mouse8" = "none"` keeps Back for the browser while
`[mouse.anywhere]` flips workspaces with it everywhere else.

A button is written by name -- `left`, `middle`, `right`, `side` and `extra`
(the two thumb buttons, which browsers take as Back and Forward), `forward`,
`back`, `task` -- or by X's number, so a Fluxbox `keys` line carries over:
`Mouse1` to `Mouse3` are left, middle and right, `Mouse4` to `Mouse7` the
wheel, and `Mouse8` on the thumb buttons and beyond. The wheel is `WheelUp`,
`WheelDown`, `WheelLeft` and `WheelRight`, and a binding acts once a notch,
a touchpad's 60 pixels counting as one. Modifiers come first, as in `[keys]`,
and `Double+` makes a double-click within `double-click-ms`; its first click is
a click like any other.

Scroll flipping goes down and right to the next workspace, as Fluxbox does out
of the box. To turn it round, as a Fluxbox `keys` file with
`OnDesktop Mouse4 :NextWorkspace` does, bind the wheel over the desktop all
four ways:

```toml
[mouse.desktop]
"Mouse4" = "next-workspace"    # WheelUp
"Mouse5" = "previous-workspace"
"WheelLeft" = "next-workspace" # sideways goes as up and down do
"WheelRight" = "previous-workspace"
```

A way left unbound is still scroll flipping's, so a tilt wheel or a sideways
swipe would go against the wheel. A binding goes by the direction libinput
reports, so with `natural-scroll = true`, `WheelUp` is scrolling up as
applications see it.

To find a button's name, run `sudo libinput debug-events` and press it:

```
event5   POINTER_BUTTON   +1.21s   BTN_SIDE (275) pressed, seat count: 1
```

`BTN_SIDE` is `side`, or `Mouse8`, and any button can be written by its code,
`Button275`. A mouse whose extra buttons arrive as keys (`KEYBOARD_KEY`) binds
them in `[keys]`.

Three things to know before binding one:

- An unmodified button in `[mouse.window]` or `[mouse.anywhere]` is taken from
  every application: `"Mouse8"` there is no longer Back in Firefox. `desktop`
  and `titlebar` take nothing from an application.
- A binding on `Mouse3` in `[mouse.desktop]` replaces the shell's root menu;
  write `"Mouse3" = "root-menu"` to keep it.
- A binding that would take the drag away -- `drag`'s modifiers with `Mouse1`
  or `Mouse3`, in `window` or `anywhere` -- is refused, naming `drag`. In
  `titlebar` it is allowed, and the rest of the window still drags.

A workspace binding acts on the monitor under the pointer, and a window binding
on the window clicked, which the click has focused. Nothing is bound at the
lock screen. An agent's clicks and scrolls never set a binding off: they go
straight to the window or surface the agent named.

### A game holding the pointer

A game turns its camera by how far the mouse moves, and keeps the pointer from
running off while it does: locked where it is, or confined to its window.
perspicax offers the two protocols it asks with: `zwp_relative_pointer_v1`,
the mouse's own motion, which keeps coming while the pointer is pinned against
the edge of the screen, and `zwp_pointer_constraints_v1`, the hold itself.
SDL3 games, and SDL2 ones on Wayland, use them directly. An X game reaches
them through Xwayland, which uses them only when both are offered: Wine's and
Proton's `ClipCursor` becomes a confine, and the hidden pointer they warp back
to the middle for mouselook becomes a lock, with the mouse's motion beside it.

Only the window in use holds the pointer: the one with the keyboard, or
whose own menu has it, on screen, with the pointer over it and inside the
part of it the game asked for. A window in the background cannot take it.
Locked, the pointer stays where it is, and only the mouse's motion reaches
the game; confined, it slides along the edge of the region. Either way it
stays the game's: a notification popping up over the game does not take it,
a click reaches the game, and no edge of the desk flips the workspace.

The person takes it back the way they leave any window: Alt+Tab or any
binding that moves the keyboard, a Logo tap or a pie or menu that takes the
keyboard, the lock screen, another workspace, or the drag modifier, whose
drag moves the window. Back in the game, a game that asked to keep the
pointer has it again at once. Let go, the pointer is where the game last drew
it. An agent cannot take it ([see acting](#what-acting-looks-like)).

## Test

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
