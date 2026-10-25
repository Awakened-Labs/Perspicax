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
perspicax-mcp          MCP server (rmcp, stdio or a Unix socket) — eleven tools, DTOs, receipts  [portable]
perspicax-index        node cache, stable ids, selectors, deltas, refusals  [portable]
perspicax-node         node schema — AccessKit types plus Origin and Visibility
perspicax-policy       WM decisions as data — focus, bindings, placement, monitors, workspaces, snapping  [portable]
perspicax-config       config.toml — schema, classic/minimal profiles, feature check  [portable]
perspicax-atspi        impl Ingest — AT-SPI2 over D-Bus
perspicax-compositor   impl HostView — Smithay: outputs, seat, damage. Headless draws nothing.
perspicax-protocols    perspicax's own Wayland protocols — the channels to the desktop shell and the settings app
perspicax-shell        the desktop — wallpaper, panel, menus, tray, icons; a Wayland client, no Smithay
perspicax-settings     the settings app — config.toml edited in place, a chord bound by pressing it; egui, behind `gui`
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
and outlives the client. Not `--session`: a display manager's session has no
agent on its stdin. A request sent before `initialize` is answered with
an error naming what to send first, and the server waits on.
`--mcp-socket PATH` serves the same on a Unix socket instead, for an agent
running inside the session it drives: see [An agent inside the
session](#an-agent-inside-the-session). Eleven tools: `window_list`,
`observe`, `resolve`, `act`, `window_close`, `tab_forward`, `window_focus`,
`window_type`, `deltas`, `screenshot` and `launch`. The four window verbs act
on a whole window by its surface: a close is a request the application may
answer with a dialog, a tab is brought forward only where the person can
already see its group, never by switching what they are looking at, and a
window with no controls to name, a terminal or a game, is focused and typed
into only while some of it can be seen. `launch` starts a program for the
agent, no shell between, and answers its pid; on a seat, only one the
person's `[agent] launch` names (see [An agent inside the
session](#an-agent-inside-the-session)).

**Nothing else is on that wire.** perspicax moves it off its own stdin and
stdout before it starts anything, onto copies no program can inherit, and
leaves `/dev/null` and its stderr in their place, so no program it starts --
Xwayland and what the shell launches included -- is given the wire, to take
the agent's requests from or to print a reply perspicax did not send on. A
program it launches, `--mcp` or not, reads nothing and prints to perspicax's
stderr: a `--spawn` program's chatter (GStreamer's `Setting pipeline to PAUSED
...`, say) lands beside perspicax's own log rather than between the replies.
So a client that pipes perspicax's stderr has to read it: what those programs
print can fill the pipe, and perspicax's logging waits on it.

What perspicax cannot move is a copy of the wire somebody else was given
before it ran, so it refuses to serve on one, before it starts anything:
- **A stderr that is the wire too** (`2>&1`, or one file for both), since
  every program perspicax starts prints there.
- **A session bus given the wire.** `dbus-run-session -- perspicax --mcp`
  gives it to the bus daemon, which gives it to every service it starts,
  including one a program asks for by name. Asking for a bus of its own
  (below) inside it is refused the same way, since the bus perspicax was
  started on holds the wire all the same.

A terminal never counts, so `perspicax --headless --mcp` typed in one is
served, and neither does `/dev/null`. A bus that will not say which process
serves it, or whose daemon perspicax cannot look into, is served with a
warning in the log: a daemon given the wire was started beside perspicax, as
its user, and answers.

**A bus of its own** is the way to run under a session bus nobody else
started. `perspicax --headless --mcp --own-bus` starts one itself: a
`dbus-daemon` that reads nothing and prints to perspicax's stderr, as every
program perspicax starts does, and that is given none of the wire. With no
session bus at all, as with `ssh host perspicax --headless --mcp`, `--mcp`
starts one without being asked. The bus lives in
`$XDG_RUNTIME_DIR/perspicax-bus-<pid>`, which is also the runtime folder of
everything it starts, and none of them is told your displays. That keeps the
accessibility bus it starts from taking your desktop's: at-spi puts its
socket in the runtime folder, and binding it removes any socket already
there. The bus ends when perspicax ends, however it ends, `kill -9` included,
and takes everything it started with it. So does the accessibility registry
perspicax starts for it. A perspicax that ends of its own
accord removes the folder too, unless one of those programs left a file in
it. A killed one leaves it behind.

Don't run perspicax under `dbus-run-session` in any form, not even with the
wire handed to it on fds 3 and 4, which `dbus-run-session` does not give its
daemon. `dbus-run-session` keeps its own copy of every descriptor it passes
on, for as long as the run lasts. A process running as you can reopen a pipe
through that copy (`/proc/<pid>/fd/4`) and write a reply perspicax did not
send, and perspicax cannot tell that copy from the client's own end. Or serve
the agent on a socket ([An agent inside the
session](#an-agent-inside-the-session)).

**Nor can a program take the wire.** By default any process running as you
may reach into any other of yours. It can reopen a pipe or a file another
holds, through `/proc/<pid>/fd/<n>`, and write a reply on it. It can take a
descriptor whole with `pidfd_getfd`, a socket's as much as a pipe's. Or it can
attach with ptrace and rewrite what the other says. Yama, on a kernel that
has it, keeps the last two to a process's ancestors, and lets the first
through. So under `--mcp` perspicax keeps every other process out of itself:
it makes itself non-dumpable, and the kernel then allows none of the three
without `CAP_SYS_PTRACE`. `perspicax attach` does the same. The cost is the
core dump, and looking at perspicax's descriptors (`ls -l /proc/<pid>/fd`)
takes root. A program it starts is not kept out with it, since an `exec`
undoes it.

The client's end of the wire is the client's to keep. A pipe the client holds
can be reopened through its own `/proc/<pid>/fd` just the same, and a socket
end cannot be. So give perspicax a socketpair, as Node's clients do, Claude
Code among them, rather than the pipe Python's `subprocess.PIPE` or Rust's
`Stdio::piped()` gives.

`window_list` lists the desk as well as the windows: a panel, a wallpaper or a
menu is `kind: layer`, with the `layer` it stacks in and the
`untrusted_namespace` its program gave it, and a screen locker's cover is
`kind: lock_cover`. A menu, a combo list or a tooltip an application opened is
`kind: popup`, with `from` naming the window or panel it hangs from: drawn
over it, and covering what is under it (see "A menu covers what is under it",
below). An X11 application's has a `from` only if it names the window it was
opened from: GTK 3's and Qt's do, Steam's and GTK 4's do not. An agent can
read and click a panel or a menu as it would a window; a window verb aimed at
one is refused as `not_a_window`. A window rolled up to its titlebar is
`shaded: true`, and one the person keeps on top of the others is
`on_top: true`, as are its dialogs and menus, kept there with it.

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
target's own rectangle, a menu the act opened, lit or closed included. Choosing
an item usually closes its menu, so an act on a menu that has gone by then is
read where the menu was drawn on its window, its closing a frame there. Weigh it
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
A node of a window rolled up to its titlebar is refused as `rolled_up`, and
so are `tab_forward` into a group rolled up and `window_focus` and
`window_type` on one, though `window_close` still closes one. None is cleared
by the agent switching the person's screen on its own, or unrolling what the
person rolled up.

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

A control of a menu is refused as `menu_unplaced`, naming the open menu as
`popup`, while its window has a menu open that perspicax has not yet matched
to the application's controls. Toolkits measure a menu's controls from
different places, and placed on the window they could be judged, and clicked,
somewhere else entirely. It clears within moments, once the application has
been read since the menu opened: resolve it again then. An X11 menu that names
no window it was opened from holds off every menu of its application, and
clears only if it is a window of its own in the application's tree, as GTK 3's
context menu is: a GTK 4 application's menus stay refused under X11 for as
long as one is open.

A control its application gives no bounds for is refused as `unplaced`.
Without a rectangle there is nothing to judge, and unlike `unjudged`, waiting
does not help while the application stays silent. A Flutter application is
the case that prompted it: its labels can be read and none of its controls
acted on. A call an application leaves unanswered costs perspicax a deadline
of one second, and a walk of its tree stops asking after three, keeping what
it has read: one silent call does not cost the rest of the tree.

Asking where a control is can cost the application more than a second.
Flutter's bridge answers by asking itself and waiting, so the question
freezes the whole application for 25 seconds, and the answer then has no
geometry in it. A client frozen while a person moves the pointer over it
stops reading what the compositor sends, and is disconnected once a megabyte
waits for it: half a minute or more of an ordinary mouse moving. So perspicax
never asks. Flutter's controls hang beneath a plug that its own process
embeds in a socket, and the application says three things at once that mark
it: the socket has no place, the plug is on the socket's own bus, and the
plug has no parent of its own. perspicax knows the plug by those, and asks
neither it nor anything beneath it where it is. Its controls are refused as
`unplaced`, and a Flutter application is never frozen by being read.

Any other control left unanswered when asked where it is ends the asking for
that read. If the application had just answered the rest of the read in full,
that control and everything beneath it are never asked again while the
application runs, and are refused as `unplaced`: an application that asking
freezes some other way is frozen once, at its first read, rather than at every
change it makes. One that was already silent, busy with something else, is
asked again at its next read.

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
focus cannot move between the two. While a window's menu holds the keyboard,
the keys go to the menu, where a letter can choose an item: typing at the
window is refused, naming the menu, and typing at the menu goes ahead.

A window whose toolkit has no accessibility bridge — foot, a game, an SDL or
plain-GL canvas — is listed by `window_list` with `nodes: 0`, and has nothing
in it for `act` to name. `window_focus` gives it the keyboard by its
`surface`, and `window_type` types into it, under `type`'s rule: only while it
holds the keyboard, and otherwise refused as `focus_elsewhere`. Focusing
changes nothing the person is looking at: the window is not raised, restored
or brought to the workspace showing, though a fullscreen one comes on top
when it takes the keyboard, as it does for the person. So it has to be in
sight already, and is refused when it is not, as its nodes would be: on a
workspace not showing, a tab behind, minimized or rolled up. It is in sight
while any of it shows, since keys aim at no pixel and the person sees what
is typed into the part that does: covered wholly, it is refused as
`occluded`, naming the window in front, and on no monitor as `off_screen`.
The receipt says whether focus landed and what the window drew, anywhere in
it, and is `gone` when what was typed closed it, `exit` into a terminal.
Clicking a canvas is still not possible: it needs a coordinate, which an
agent is never given.

While a game holds the pointer ([see Mouse](#a-game-holding-the-pointer)), an
agent's `click` and `scroll`, on the game or anywhere else, are refused as
`pointer_captured`, naming the game as `captured_by`: they would move a pointer
that may not move. So are its `focus`, `window_focus` and `tab_forward` on any
other window, which would take the game's keyboard and the pointer with it.
Only the person ends a hold, so waiting for them to pause does not help; the
game itself can still be focused and typed into.

While a window's menu holds the pointer and keyboard, one the person opened
with a right-click say, an agent's `click` and `scroll` on any other
application are refused as `menu_open`, naming the menu as `popup`. A press
there would close the menu under the person before it reached what was aimed
at, and a scroll there would reach nothing. So are `focus`, `window_focus` and
`tab_forward` on anything but that window and its menus: the menu keeps the
keyboard, and a tab brought forward would be raised over it, or put its window
away and the menu with it. The menu's own application can still be clicked
and scrolled anywhere, as the person can, so choosing an item goes ahead, and
so does typing at the menu. `window_close` still asks any window to close. The
compositor makes the check in the same turn it would act, and it clears as
soon as the menu closes, chosen from or dismissed.

**A click is judged twice.** The gate judges a node against the facts as they
stood when the agent's call read them, and the compositor carries the act out
a moment later, on its own thread. In between, a menu can open over the node,
often one the agent's last act opened, or a window be raised over it, or the
node move out of view. A click aimed at the node's own surface would still
reach it, through what the person now sees there. So the compositor judges a
`click` or a `scroll` again, against its own facts as they stand, in the same
turn that sends the events, and refuses it as the gate would: `occluded`,
naming what covers the node now, `clipped`, `off_screen` and the rest. Nothing
is sent, and the facts the agent reads next already show what was in the way.
`focus` and `type` aim at no pixel and are not judged again; typing checks the
keyboard instead.

**A titlebar is pixels no client drew.** The frame perspicax draws around a
window goes into the facts beside the window, so a node of another window
under a titlebar is refused as `occluded`, naming the window the titlebar
belongs to, and the verdict is proof rather than policy: the frame is solid
to an agent, even drawn see-through for a person (see [See-through
windows](#see-through-windows)). A window's own frame sits outside it and
never covers its own nodes. A window rolled up covers by its titlebar alone:
where its client was, what is under it is uncovered, as the person sees it.
One cost of drawing the frame: a toolkit that would have drawn close and
maximize buttons no longer does, so they are not in its accessibility tree;
`window_close` and `tab_forward` are the agent's way to do what they did.

**A window covers everything it draws.** A client says where its window is
inside the picture it draws, and that is where perspicax places it and
measures its nodes from. It is not how far the window reaches. The shadow a
toolkit draws around its window, and a subsurface hanging past it, are drawn
over whatever is under them and take the person's clicks there. And a
client can declare its window as small as it likes. So a node of another
window under any of it is refused as `occluded`, naming the window, and
`window_list`'s `bounds` (the window inside its shadow) can stop short of
what covers a node. Where the client says it is opaque, the verdict is
proof. Anywhere else it draws, a shadow, a part drawn see-through or a
window from a client that says nothing, it is policy, and `--dump-tree`
counts it as `unproven`. An opaque region is believed in that one direction
only: a client that leaves part of itself out of it has not said that part
is see-through, and gains nothing by saying less than it paints. A node
just beside a window can therefore be refused under its shadow, though a
person can make it out and click it. The person can bring its window
forward; an agent can't yet
([#147](https://github.com/Awakened-Labs/Perspicax/issues/147)).

**A menu covers what is under it.** A menu, a combo list or a tooltip is a
popup: a surface of its own, drawn directly above the window or panel it hangs
from, where it takes the pointer. So a control under an open menu is refused as
`occluded`, naming the menu, whether it is another application's or one of the
menu's own window: a click there would land on the menu. One that holds the
pointer and keyboard also keeps an agent's acts off every other application
until it closes, refused as `menu_open`. And a menu's own
items are judged inside the menu, an item hanging past its window's edge
included, and an agent's click on one lands on the menu. Toolkits disagree
about where they measure a menu's items from: GTK 3 from the menu itself, GTK
4 from its window. Perspicax tells which by where the items land in each, and
joins a menu to its items only when exactly one puts them on the menu; until
it can, its window's menus are refused as `menu_unplaced`. A menu of a window
that has drawn nothing yet covers what is under it all the same. An X11
application's menus are popups too
([#159](https://github.com/Awakened-Labs/Perspicax/issues/159)), drawn where
they put themselves, over every window as they open. GTK 3 and Qt name the
window a menu was opened from, and their menus are joined to their items as a
Wayland application's are. Steam and GTK 4 name none, and such a menu is
joined only to items that are a window of their own.

**Text an application rendered is marked as such.** Every string reaches an
agent under `untrusted_text`, beside the credentials of the process that drew
it — the marking is in the key, so it cannot be skimmed past, and the provenance
is at the point of use rather than in a preamble a model has to have remembered.
That is this project's injection defence, and it is a read-path property rather
than an act-path gate. It holds because nothing but perspicax writes on the
agent's wire: perspicax refuses a wire something else was given first, and
keeps any other program from taking it, so a program it started printing a
line of JSON-RPC cannot pass it off as perspicax's own (see [What acting looks
like](#what-acting-looks-like)).

**Consent is about the person, not the node.** Headless, there is no capability
gate: perspicax is one actuator among several, an agent refused a click runs the
command instead, and every client is on a socket the agent set up. On `--seat`
the question has two answers, because the person launched most of what is on
screen. There an agent may act only on what it was granted
(`Refusal::NoCapability` otherwise), and not at all while the person is using the
keyboard or pointer, or has a mouse button down, however long they hold it
still: an act in the middle of their typing would race it, and one in the middle
of their drag would carry the window off to where the agent aimed and end the
drag under their hand. Typing only into a window that holds the keyboard is
what keeps an agent's keys out of the person's terminal while it acts on a
window it was granted. A window with no
accessibility tree has no node to name, and is typed into by its surface
instead, with `window_focus` and `window_type`, under the same rule and the
same consent.

**What is granted, and for how long.** A grant is to a process, and lasts
until that process exits: its pid is the kernel's to give to the next program
once it has gone, so a grant that outlived it could pass to anything. Four
things grant one:

- **`--spawn`**, for what it starts;
- **Logo+A**, the person handing the focused window's program over; Logo+A
  again takes it back, and Logo+Shift+A takes back every grant there is,
  `--spawn`'s included;
- **`agent = true`** on a binding or an `autostart` entry, for what it
  starts;
- **`launch`**, the agent starting a program for itself, which the person's
  `[agent] launch` must name. A launch covers what that program starts as
  well, while it runs, so `flutter run` covers the application it builds. It
  covers what descends from it and nothing else: a program that forks twice
  to leave its parent, one D-Bus starts, or one that hands its window to a
  copy of itself already running is not covered.

Everything else is exactly the one process. An X11 window is never granted
by a key: every X client shares one Xwayland, and may read and drive the
others, so granting one would grant them all. And the settings app is never
an agent's, whatever grants it: it writes the file that holds an agent back
([see The settings app](#the-settings-app)). `window_list` says which windows
an agent may act on (`agent`), and `deltas` says when that changes. On screen,
each is marked out: a line round it, in front of its shadow, and its titlebar,
where perspicax draws one, in the agent's colours (`[decorations.agent]`).
Headless, where every window is an agent's, nothing is marked.

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
reading applies to it. A picture of the monitor shows a window rolled up as
the person sees it, its titlebar and what is under the rest. A picture of
that window alone shows it whole, as one of a minimized window does: it is
a reading of the window, which the agent may read already, and without the
shadow it casts. A picture never shows a window halfway rolled up: what is
pictured, and what an agent is told, is where it is going, from the moment
it sets off.

**A frame's looks change nothing an agent is told.** Its strips are proven
to cover what is under them, square: a pixel outside a rounded corner's arc,
where nothing is drawn, still counts as frame, which errs on the side of
refusing. The shadow perspicax casts under a window it frames covers nothing:
perspicax drew it, and knows it is see-through, so a node under it can be
acted on, and a click there lands there.

## An agent inside the session

`--mcp` has one client, whatever started perspicax, and on a seat that is the
login. So the agent a person most wants beside them, one they can see and talk
to in a terminal inside the session, cannot reach it: it would have to start
the very session it is running in. `--mcp-socket` serves the same eleven tools
on a Unix socket instead:

```sh
perspicax --seat --mcp-socket "$XDG_RUNTIME_DIR/perspicax-mcp" --spawn foot
```

A session the display manager starts is given no flags, so the config says
it instead: `[agent] socket = true` serves the socket at
`$XDG_RUNTIME_DIR/perspicax-mcp`, a path serves it there, and `false` serves
none, whatever `--mcp-socket` says. The config goes over the flag, and is read
once, as the session starts: a change waits for the next login.

**What it may start.** `[agent] launch` names the programs an agent may start
for itself with the `launch` tool, and then act on: `"off"`, which is the
default, `"any"`, or a list. A bare name admits that word as the program and
nothing else, and is found in the session's `PATH`, in the folders there that
are whole paths; a path admits that file alone, so `flutter` does not admit
`/tmp/x/flutter`. **What a launched program starts is the agent's too**, while
it runs, so listing a terminal, a shell or an interpreter -- `foot`, `sh`,
`python3` -- admits everything it can start: list the programs themselves. A
list a reload narrows applies to the next launch, and what was launched before
stays granted until it exits or Logo+Shift+A takes it back. A launch refused
says why: `not_admitted`, naming what the list admits, or `not_started`.
Headless reads no config, and an agent there may launch any program, as every
protocol there is open.

Every program the session starts is told where the socket is, in
`PERSPICAX_MCP_SOCKET` (empty in a session that serves none), so an agent
started inside it needs only a bridge from its stdio to the socket, and
`perspicax attach` is one. For Claude Code in that terminal:

```sh
perspicax claude-code install
```

This puts `perspicax attach` in Claude Code's user config, which every project
reads, through Claude Code's own command line: `claude mcp add-json -s user
perspicax '{"type":"stdio","command":"/usr/local/bin/perspicax","args":["attach"]}'`.
Claude Code finds it the next time it starts, and `--dry-run` says what would
be run. `perspicax claude-code status` says whether it is there and runs, and
`uninstall` takes it out.

A server that runs `perspicax attach` counts as perspicax's whatever it is
called, so one added by hand -- `claude mcp add perspicax -- perspicax attach`
-- counts too. An install leaves one that works as it is, and mends one whose
program has gone, keeping its name and the rest of its entry. It adds no
second one, since the desktop holds one conversation at a time. Two ways in,
or a `perspicax` that is something else, are refused, naming them.

ennius starts an MCP server only for a session granted one of its tools, and
denies a session every tool it was not granted. For ennius:

```sh
perspicax ennius install
```

This adds `perspicax attach` to ennius's `[tools.mcp]`. It also grants every
tool perspicax serves, as `mcp:perspicax:act` and so on, to the sessions that
`ennius tui`, `chat` and the GUI start (`[interactive.grant] tools`). Both
steps go through ennius's own `ennius tools mcp add` and `ennius config grant
add`.

- **No grant into ennius's defaults.** A config that does not list
  `[interactive.grant] tools` is given no grant. ennius writes a grant as the
  whole list, so a grant written there would replace ennius's own default
  tools rather than add to them. List them there first.
- **Another name is kept.** A server already wired under another name keeps
  that name, and its grants go under it.
- **Status and uninstall.** `perspicax ennius status` says what is missing,
  and `uninstall` takes out the server and its grants.
- **Inside the session only.** `attach` finds the session through
  `PERSPICAX_MCP_SOCKET`, so ennius reaches the desktop when it is started
  inside the session, from a terminal or the start menu, and not otherwise.
- **Older ennius.** An ennius from before its #193 takes up an `mcp:` grant in
  `agent run` only, and `status` says so.
- **One at a time.** Claude Code and ennius wired together take turns at the
  desktop's one conversation.

`perspicax attach` passes the client closing its stdin on as the end of the
conversation, and ends when the session closes the connection. Like `--mcp`, it keeps every
other process out of itself, so no program can take the wire through it.
`perspicax attach PATH` names a socket instead. Anything else that joins stdio to a Unix socket will do, with
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
running as you can then drive whatever the agent may -- on a seat, what is
granted to it. `$XDG_RUNTIME_DIR`, which only you can enter, is the place
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
a surface of its own, outside the window's tree of subsurfaces. Its frames are
its own, for an act on one of its items, and the window's too, placed where the
menu is drawn on it, past the window's edge if it hangs there, for an act that
opened it.
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

Leaving the flag out does not fail the build. It quietly makes a compositor
that runs headless only and a shell that draws nothing. Export the features
once instead, and `scripts/build.sh`, which is `cargo build` reading them,
builds with them every time:

```sh
export PERSPICAX_BUILD_FEATURES=perspicax/desktop,perspicax-shell/full
scripts/build.sh --release                 # builds with them ...
scripts/build.sh --release --features ''   # ... and this one without
```

Every argument goes to `cargo build` as it is, and features on the command
line win over the variable. The variable is spelled as `--features` is:
comma-separated, with a crate's feature written `crate/feature`. The script
reads it, not cargo, which takes features only from its command line, so a
bare `cargo build` still builds without them.

perspicax starts `perspicax-shell` from beside its own binary, and from `PATH`
if there is none there. A workspace build without `perspicax-shell/full`
leaves a shell of no components in `target/`, which draws nothing; to install
the shell on its own, `cargo install --path crates/perspicax-shell --features
full`.

The settings app is built in a cargo run of its own:

```sh
cargo build --release -p perspicax-settings --features gui
```

Its window's toolkit puts every Wayland client in a build onto libwayland,
and cargo turns a feature on for a whole build, so a workspace build leaves
the app out and keeps the shell off libwayland. It needs no development
packages: libwayland, libEGL and xkbcommon are opened as it starts. What it
writes is checked by the `perspicax` beside it, or else the one on `PATH`, so
install the two together.

Before the first session, ask the machine what it lacks:

```sh
perspicax doctor
```

It prints a row for each thing a session needs, each found the way the
session would find it:

- the runtime folder;
- a session bus, and the programs that start one;
- the accessibility registry;
- the shell;
- Xwayland and `/tmp/.X11-unix`, where the config starts Xwayland;
- the config, in the words of both programs that read it;
- the programs the menus' ways to leave run;
- the agent socket.

Then it says whether Claude Code and ennius are told how to reach the desktop
([An agent inside the session](#an-agent-inside-the-session)). A warning names
what puts it right. The exit status is 1 only when something a session cannot
do without fails, such as a missing Xwayland when the config starts it.
Nothing is started to find any of this out. `perspicax doctor PATH` asks about
another config.

Run it from a text console (a TTY, not a terminal inside another desktop), with
seatd or logind managing the seat. Log to a file: the console the session
takes over shows nothing until it ends, and what the programs it starts
print goes to the same file. With no `RUST_LOG`, a seat and its shell log
their warnings; `RUST_LOG=info` says what they are doing as well.

```sh
perspicax --seat --spawn foot 2>~/perspicax.log
```

Two chords always work, whatever the configuration says:
**Ctrl+Alt+Backspace** ends the session, and **Ctrl+Alt+F1…F12** switches VT.
No binding can take them, so `[keys]` refuses either, `"none"` included.

X11 applications run under an Xwayland the session starts (`xwayland = false`
in the config turns it off), with `DISPLAY` set for everything it launches:
`--spawn` programs and the autostart list start once Xwayland is ready.
Headless starts none, so what CI runs needs no X server, unless
`--xwayland` asks for it: `perspicax --headless --xwayland --spawn xterm`
hosts an X11 program for an agent, in a build with the `xwayland` feature.
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
an activation token for: the X time of that input, under ten seconds old,
and only while an X11 window has the keys (#256). Any X program can learn
the X server's time, so it is believed only from inside the X world in use,
as a token is only from the program with the keys; otherwise an X11 window
could take the keys from a Wayland window the person is typing in. So a
program started again from a Wayland terminal or launcher comes up raised,
to be clicked. Wine sends no time, so its window comes back without the
keyboard. Without input, or with nothing to vouch for it, a window on a
workspace that is not showing waits there, since showing that workspace
would move the keyboard. Asking with `_NET_WM_STATE_HIDDEN` minimizes or
brings back, as Openbox takes it. And the root's `_NET_ACTIVE_WINDOW` names
the X11 window with the keyboard, or none while a Wayland window has it,
which Wine goes by for the window in front.

An X11 application's menus, tooltips and drag icons place themselves: they
bypass the window manager (override-redirect, in X's terms), and are left to
their application, as any X11 window manager leaves them. Each is drawn where
it puts itself, over what it covers, and the pointer reaches it. But it is
never focused, raised, moved or closed, nor listed on a taskbar, no binding
acts on it, and it
never takes the keyboard: not when the pointer reaches it, whatever the focus
model, not when it is clicked, and never as the next window when one closes,
a workspace switches or Alt+Tab cycles. Its keys reach it through the window
it was opened from, which keeps the keyboard and stays the window in use, lit
and undimmed, while the menu is open. Given the keyboard, as before #181, the
window lost it and the application closed its menu: Steam's closed as soon as
the pointer reached one. An agent sees each as a popup, as it sees a Wayland
application's menu (#159): hung from the window its `WM_TRANSIENT_FOR` names,
through any menus between, or from nothing when it names none. Unlike a
Wayland application's, it is in no window's picture, and only a picture of its
monitor shows it. Two things are left:
- under sloppy or strict focus, a menu still closes if the pointer crosses
  another window on its way to an item (#183);
- an X program that bypasses the window manager to take the keyboard for
  itself -- dmenu or rofi on X11, an old fullscreen game, Wine with its
  windows unmanaged -- types only while one of its own X11 windows has the
  keyboard (#182).

A Wayland client may take its window off the screen without closing it, by
attaching no buffer to it, as xdg-shell allows, and ask for it again later.
To everyone else the window has closed. It leaves the screen, the taskbar
and `window_list`, its menus are dismissed, and a move or resize of it ends
with nothing in hand. With a person at the seat, the keyboard goes to the
window now on top. Asked for again, it is configured as a new window is,
with what it was told before (maximized, fullscreen) forgotten, as
xdg-shell says. It is back from the first picture it draws, and not before.
It returns as a new window: placed, given the keyboard, announced to the
taskbar, and with a new id. An X11 window its client withdraws and maps
again goes and comes back the same way.

A panel, a launcher or any other layer-shell surface may be taken off the
screen the same way, by attaching no buffer to it, as layer-shell allows.
To everyone else it has gone, as one destroyed has. The room it kept is given
back, and a window that fills its monitor fills it again. Its menus are
dismissed, the keyboard goes to the window on top if it had it, with or
without a person at the seat, and an agent can no longer reach it. Asked for
again, it is configured as a new one is, keeps its room again from the
commit that asks, and returns with a new id. One whose monitor is unplugged
while it is away is told it is closed. A launcher that asks for every key,
new or back, has them from the first picture it draws, and not before.

And it keeps them while it is up, as layer-shell says the top-most one that
asks for every key does: fuzzel, say, or the shell's own menus and pies. A
window that opens under it, asks to be brought forward, or is clicked,
pointed at or picked in the taskbar is raised as it would be, but takes
none of the keys typed at the launcher. It has them once the launcher goes,
being on top then. Only another launcher takes them, the newest being the
top-most (#131).

Nor does a token any program made bring a window forward with the keys. An
application asks for that with an activation token from the program the
person used to start it or to open a link in it: a launcher, a terminal, a
window of its own. The token brings the window forward only if the program
that made it had the keys as it did, through any of its windows or menus,
and for ten seconds at most (#187). One any other program makes is
forgotten as it is made, whatever input it says it answers, since that is
anything a program cares to name. Otherwise a window could come forward
while the person typed in another, a password at a terminal's `sudo` prompt
say, and take the rest of it, Enter and all. For the same reason a menu that
asks for the keys is dismissed when its program has none of them: one opened
with a right-click still has them, since the click gives the window clicked
the keys before it reaches it.

And a token is good only until the person moves on (#257): until they press
a key or a button, or the keys go to another program, whatever moves them --
a window that opens, a click, Alt+Tab, the pointer under sloppy focus, a
workspace switch, the lock screen. Otherwise the program they were using a
moment ago could make one as they left it and spend it a few seconds later,
on whatever they had moved on to. The keys coming back from a launcher as it
goes are no move: that is when the program it started spends the token it
was handed. A notification or a dock never has the keys, so its token is
vouched for instead by the click it was just sent: the person's latest
press, under ten seconds old and named exactly. So clicking a notification's
action brings its program forward. A program keeps no more than eight tokens
at a time, so one making them in a loop has nowhere to grow. Left open: a
program may open a launcher of its own, a layer asking for every key, and it
has the keys as it opens (#259), so a token it makes there outlives it. And,
as on wlroots, a program that laid an invisible layer over the screen would
be sent the clicks meant for what is beneath it, and could make tokens with
them, though nothing beneath would answer the person's clicks while it was
there.

A new window opens on the monitor under the pointer, in the room its panels
leave, cascaded from the top-left corner: 32 pixels further in for each
window opened before it, back to the corner before the middle. With a
person at the seat it is then put wholly inside that room, frame and all,
as soon as its size is known: an X11 window as it maps, a Wayland window at
its first picture. One that runs off the right or the bottom slides back on
at its own size. One still too big goes to the left or top edge and is
given all the room there is, never less than its program allows; one whose
least is bigger than the room keeps its titlebar on screen and hangs off
the right and the bottom. A Wayland window is told that room before it
first draws (`configure_bounds`), so a toolkit that reads it, as GTK 4
does, opens at a size that fits. An X11 window whose hints say it asked for
its place (USPosition or PPosition), as a program that puts itself back
where it was last time says, opens there. That is its own top-left corner,
not our frame's, on the monitor most of it is on, or the one under the
pointer if that monitor is gone, and it is slid and shrunk onto it the same
way. A Wayland window has no way to ask for a place, so its program's last
one is remembered instead (below). All this is done once, as a window opens:
after that, where it goes is the person's, and a window dragged partly off
the screen stays there. With nobody at the seat, a window stays where the
cascade put it, at the size it chose.

With a person at the seat, a program's next window opens where its last one
closed, so a Wayland program, which cannot ask, still opens where it was
left. A program is known by its Wayland app id or its X11 `WM_CLASS` class,
and what is remembered is the monitor its last window was on and where that
window's corner was on it: where it would have been put back, for a window
closed maximized, snapped or minimized, and where its group was, for a tab
behind another. Not its size, which a program chooses as it first draws,
and not its workspace: it opens on the one showing now. A place an X11
window asks for itself comes first, the cascade only after, and either way
the window is then put wholly inside its room as any new window is. If that
monitor has been unplugged, it opens at the same corner of the one under the
pointer. A second window of a program, opened while the first is still
there, steps 32 pixels aside. A dialog is never remembered, nor put where its
program was: an xdg window with a parent, or an X11 one transient for
another, modal, or of any type but normal. Windows still open as the session
ends are remembered as though they had closed. With nobody at the seat,
nothing is.

Where programs' windows closed is kept between sessions in
`$XDG_STATE_HOME/perspicax/windows.json` (`~/.local/state` without it),
beside the session's log, and only you can read it. It names programs by app
id and monitors by connector, never a window's title, which carries document
names and URLs, and holds the 200 programs closed most recently. It is your
history, not the state of a window, so none of it reaches an agent: not
`window_list`, not anything the index is told. `[placement] remember =
false` turns it off and deletes the file; `forget` lists programs never to be
remembered, by app id or `WM_CLASS` class matched exactly, and forgets what
was kept of them.

The session reads `$XDG_CONFIG_HOME/perspicax/config.toml` (or `--config PATH`),
and reads it again whenever it is saved, by an editor or by [the settings
app](#the-settings-app). Without one it runs the `classic` profile, which is
Plasma's and Windows' habits:

- click to focus, Alt+F4, Alt+Tab;
- Alt+drag to move and Alt+right-drag to resize;
- four workspaces in a row: Ctrl+Logo+arrows switches, add Shift to take the
  focused window along, Ctrl+F1…F4 goes straight to one;
- dragging a window to an edge of the desk snaps it to half the monitor, a
  corner to a quarter, the top to maximized, with Logo+arrows to do the same
  from the keyboard;
- Logo+Shift+Left/Right moves a window between screens, Logo+Shift+R reloads;
- Logo+A hands the focused window's program to an agent, or takes it back, and
  Logo+Shift+A takes back everything an agent was granted;
- a tap of Logo on its own asks the desktop shell for its start menu;
- a titlebar drawn by the compositor for a client that asks for one (Qt, foot,
  GTK 3 without a headerbar, and X11 applications): drag it to move,
  double-click it to maximize, and roll up, minimize, maximize and close at
  its right; drag an edge or a corner, or just outside one, to resize. Its
  top corners are rounded and it casts a soft shadow, as Plasma's do, a
  button lights up under the pointer, and a window rolls up and down rather
  than jumping;
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
It does not snap. The wheel over a titlebar rolls its window up to it, and
down again, as Openbox does. Its titlebar is Fluxbox's: a sticky button at
the left, which keeps the window on every workspace, then a triangle, which
keeps it on top of the others, and roll up, minimize, maximize and close at
the right. It is flat, as Fluxbox's is: square
corners, no shadow, and nothing animated, though a button still lights up
under the pointer.
In both, a window being moved or resized stays in hand: switching
workspace by a key, a number or a pager takes it along, as an edge does one
being moved (resizing up to an edge never flips), sending it to another
workspace takes the person with it, and one minimized while held stays
minimized. A tab dragged with the middle button cannot come along without its
group, so a switch calls the drag off, and the tab waits with its group where
it was picked up. In both, a window's own menu, a right-click menu say, holds
the keyboard while it is open, and a key on the window still works: Alt+F4,
Alt+Tab, Logo+arrows and the rest close the menu first, as a click outside it
would, then act on the window. A key that is not about the window, a layout
switched, leaves the menu open. Switching workspace acts on the monitor of
the window whose menu is open, and a window left behind takes its menu with
it, so the keys go to whatever is in front where the person went. In both,
too, a fullscreen window
covers the panels while it is the one in use, and goes back under them when
another window, the start menu or a launcher takes the keyboard. A menu of
its own, a video's right-click menu, leaves it over them, while it is open
and as it closes. While it covers them, its edges
are its own: resting the pointer against one does not flip, so a game turning
its camera stays where it is. So are a window's that holds the pointer,
fullscreen or not ([see Mouse](#a-game-holding-the-pointer)). Nor does an edge
flip behind the lock screen.

In both, a window can be rolled up to its titlebar and back, as Fluxbox and
Openbox shade one: by the shade button, left of minimize, or by `shade`,
`unshade` and `toggle-shade` bound to a key or a button. Rolled up, it keeps its size
and its place, and is put back exactly as it was, for its client is neither
told nor resized. A click where it was lands on what is under it. It moves
by its bar, has no edge to resize by, and its menus close. It keeps the
keyboard, so what is typed still reaches it, and focusing it does not unroll
it. Going fullscreen unrolls it; maximized, snapped, minimized or on another
workspace, it stays rolled up. A tab group rolls up as one, and a tab taken
out of it comes out unrolled. Only a window whose frame perspicax draws can
roll up: one that draws its own has no titlebar of perspicax's to roll up to,
and no button. An X11 window rolls up too, but is not told it has
(`_NET_WM_STATE_SHADED`).

In both, a window can be kept on top of the others: by `toggle-on-top` bound
to a key, which neither profile binds, or by the triangle beside sticky on
`minimal`'s titlebar, filled while the window is kept. It is drawn in front of
every window that is not kept on top, whichever of them is raised or has the
keyboard, and of two kept on top, the one raised last is in front. Nothing
else changes: Alt+Tab walks every window in turn as it did, each taking the
keyboard, and a window closing gives the keyboard to the one raised last. Its
dialogs and menus are kept on top with it, and let go with it. The panels stay
over it, and a fullscreen window covers it while it is the one in use. A tab
group is kept on top as one, and a tab taken out of it comes out among the
others. A menu of a window that is not kept on top opens behind one that is,
where they overlap (#234). An X11 window is kept on top as any other, but is
not told it is, and an X11 application cannot keep a window of its own on top
by asking (`_NET_WM_STATE_ABOVE`, #236).

In both, a workspace switch slides: the new workspace comes in from the side
the person went towards, across a wrap too, and along the grid for a switch by
number, a pager or a scroll. `[workspaces.transition]` picks another effect:
`crossfade`, the old workspace's windows fading out as the new one's fade in;
`wipe`, a black bar sweeping down the screen with the new workspace above it
and the old one below; `carousel`, every other workspace sliding past the long
way round before the one asked for, so going from 2 to 3 of four passes 1 and
4, and from 3 to 2 passes 4 and 1; or `none`, at once. Each has its own length,
200 ms for a slide or a crossfade, 300 for a wipe and 400 for a carousel, and
`duration-ms` sets another. Only the picture moves. The windows are where they
will be from the first frame, so a key or a click ends the transition and lands
where it is drawn, and an agent, a screenshot and a screen recording all see
the workspace arrived at. The panels, the wallpaper, a sticky window and a
window being carried stay where they are. With a wallpaper per workspace, the
new one replaces the old as the shell draws it, a frame or two in. A crossfade
fades each window on its own, so halfway through, windows show through one
another. `[theme] motion = "reduced"` makes every switch instant, whatever
`[workspaces.transition]` says, and so is every other animation.

In both, a titlebar's buttons are a person's to choose, and where:
`[decorations] left` and `right` list them, from the bar's left, with the
title or the tabs between, as Fluxbox's `titlebar.left` and `titlebar.right`
do. There are six, `shade`, `sticky`, `on-top`, `minimize`, `maximize` and
`close`, each at one end at most, and either end may have none. Writing one
end alone keeps the profile's other, but for any button the written end took.
A bar too narrow for them all keeps close the longest, so a narrow window can
still be closed, and lets the others go from beside the title outwards, the
left-hand one first of two as near it. A button acts on its own window,
whether or not it has the keyboard. Each that toggles shows which way it is:
the sticky pin filled while the window is on every workspace, the on-top
triangle filled while it is kept on top, maximize as two squares, restore,
while it is maximized, and shade with a chevron pointing down while it is
rolled up. Under the pointer, a button is lit behind its shape, in the bar's
own ink, faintly, and less faintly pressed; close goes red,
`title-close-hover` in the palette. A press dragged off a button goes out, and
does nothing.

A button can be drawn from a picture of your own, by what a press of it then
does: `[decorations.icons]` names a PNG, or an SVG in a build with the `svg`
feature, for `close`, `minimize`, `maximize`, `restore` (maximize, while
maximized), `shade`, `unshade`, `stick`, `unstick`, `on-top` and `off-top`
(on-top, while kept on top). A picture is a mask by default, drawn in the
bar's ink by its alpha, so it reads on every bar and in every theme;
`tint = false` draws it in its own colours. A PNG is shrunk to its button,
never enlarged, and an SVG drawn at its button's own size, so it is crisp at
any scale. One that cannot be read is said once in the log, and the button
keeps its own shape, never an empty square. They are read again at every save,
found as a wallpaper is.

`corner-radius` rounds a titlebar's top corners, square for a maximized
window, whose bar meets the screen's own corners, and `[decorations.shadow]`
casts a shadow under a window perspicax frames, not under one that draws its
own frame, which casts its own, nor under a maximized or fullscreen one.
`[decorations.animation]` says how long rolling a window up or down takes,
and a button's light coming up or going out, 0 for at once.

Every key below is optional and overrides the profile one setting at a time.
A misspelled key, or a key for a feature this build left out, is refused
with its name rather than ignored. So is a chord `[keys]` writes twice, in
two spellings, since only one of them could be the binding: `"Super+a"`
beside `"Logo+a"`, or `"Super"` beside `"Logo"` for the tap. And so is a
chord no binding can take, Ctrl+Alt+Backspace or Ctrl+Alt+F1…F12.

A file with a key refused is refused whole: the session carries on with the
config it had, and says why only in its log. `perspicax config check` asks
first, in the words of both programs that read the file, each as it was
built: perspicax itself, and the perspicax-shell the session would start,
beside it or on `PATH`, asked with `perspicax-shell --check`.

```sh
perspicax config check                # the session's own file
perspicax config check ~/try.toml     # or any other
```

It prints what each said, and exits 0 when both take the file, or 78 when
either refuses it, naming the key and why. It exits 66 for a file named that
is not there, and 1 for a check it could not make. With no file where a
session would look, it says a session there runs the classic profile, and
exits 0. A shell it cannot find, or one too old to be asked, is said to be,
and `[shell]` is checked as if the shell had every component. A keyboard
layout xkb cannot make is not caught yet: the file is taken, and the
keyboard keeps its old layout, with a line in the log (#215).

```toml
profile = "minimal"            # or "classic"; minimal focuses under the pointer
drag = "Logo"                  # the drag modifier; "none" turns drags off
autostart = [["mako"], ["nm-applet", "--indicator"],
             { spawn = ["foot"], agent = true }]   # an agent may act on it
xdg-autostart = true           # and the autostart folders' entries; see below
                               # (every key outside a table goes up here)

[focus]
model = "sloppy"               # click | sloppy | strict
autoraise = false

[keys]
"Logo+Return" = { spawn = ["foot"] }
"Logo+d" = { spawn = ["fuzzel"] }
"Logo+Shift+Return" = { spawn = ["foot"], agent = true }   # granted till it exits
"Alt+F4" = "none"              # hand a profile's chord back to the client
"Alt+F1" = "root-menu"         # or "start-menu": the desktop shell's menus;
                               # { pie = "<name>" } opens a pie, see Pies
"Logo+F12" = "volume-up"       # or "volume-down", "volume-mute", "mic-mute":
                               # the shell's; both profiles bind the XF86Audio
                               # keys to them, see The mixer
"Logo+t" = "toggle-on-top"     # keep the focused window in front of the
                               # others, or let it go; neither profile binds it
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
while-dragging = true          # and take the window being moved along
scroll = true                  # scroll over the desktop: down and right to the
                               # next one; see Mouse to turn it round

[workspaces.transition]
effect = "slide"               # "slide", "crossfade", "wipe", "carousel", or
                               # "none" to switch at once
duration-ms = 200              # 1 to 2000; each effect has its own if left out

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

[placement]
remember = true                # a program's next window opens where its last
                               # one closed; false forgets every one
forget = ["org.keepassxc.KeePassXC"]
                               # never remembered: app ids, or WM_CLASS classes

[decorations]
mode = "server"                # draw titlebars for clients that ask; "client"
                               # tells every client to draw its own
title-height = 24
border = 2
focused = "#2d6fa3"            # the title is written in black or white,
unfocused = "#475057"          # whichever reads on the colour
left = ["sticky"]              # the titlebar's buttons at its left end, and
right = ["shade", "minimize", "maximize", "close"]  # at its right
corner-radius = 5              # 0 to 16 pixels; 0 is square

[decorations.shadow]           # under a window perspicax frames
size = 18                      # how far it reaches, 0 to 64; 0 is none
offset = [0, 4]                # how far it falls, right and down
opacity = 50                   # how dark, at its darkest: 0 to 100 percent
colour = "#000000"

[decorations.animation]        # how long each takes, 0 to 1000 ms; 0 is at once
shade-ms = 150                 # rolling a window up or down
hover-ms = 80                  # a button's light coming up or going out

[decorations.icons]            # a picture for a button, by what a press does
close = "~/.config/perspicax/buttons/close.png"
restore = "buttons/restore.svg"  # SVG needs the svg feature
tint = true                    # drawn in the bar's ink; false, its own colours

[decorations.agent]            # a window an agent may act on, on a seat
focused = "#5a2d82"            # its titlebar, where perspicax draws one
unfocused = "#3b2454"
outline = 3                    # a line round it, 0 to 8 pixels; 0 is none
outline-colour = "#b37feb"

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
output-management = ["kanshi", "wlr-randr", "perspicax-settings"]
shell = ["/usr/local/bin/perspicax-shell"]
settings = ["/usr/local/bin/perspicax-settings"]

[agent]                        # what an agent may do on the seat; see below
socket = true                  # serve it at $XDG_RUNTIME_DIR/perspicax-mcp;
                               # or a path, or false for none
launch = ["flutter", "zenity"] # what it may start for itself: a list, "any",
                               # or "off", the default
```

`[protocols]` names the programs that may use the protocols reaching past
their own windows: a taskbar's list of windows (`foreign-toplevel-list`, and
`foreign-toplevel-management` to activate and close them), a pager
(`workspace`), a screenshot tool (`screencopy`), a display tool
(`output-management`), the desktop shell (`shell`), which is told when a
key asks for a menu or a pie or changes the volume, and what is made of each
window, and may end the session, or make a window sticky, keep it on top, roll
it up or send it to a workspace, and the settings app (`settings`), which hears the next chord pressed, to
bind it. Listing windows and workspaces is open by default; reading pixels,
moving monitors, speaking for the shell and hearing a chord are for the
usual programs, by name. The settings app is among the display tools, to
list the monitors on its Outputs page; it changes them only through the
file. A name is whatever the kernel says the client is running, which any
program can be called, so a full path is the stricter form. Whatever this
says, all of them are inert while the screen is locked.

| Protocol | `[protocols]` key | Spoken by | While locked | When the rule narrows |
|---|---|---|---|---|
| `ext-foreign-toplevel-list-v1` | `foreign-toplevel-list` | window lists | nothing new is told; told on unlock | every window closed, list finished |
| `wlr-foreign-toplevel-management-unstable-v1` v3 | `foreign-toplevel-management` | waybar `wlr/taskbar` | requests ignored | every handle closed, manager finished |
| `ext-workspace-v1` | `workspace` | waybar `ext/workspaces` | switches ignored | everything removed, manager finished |
| `wlr-screencopy-unstable-v1` v3 (shm) | `screencopy` | grim, wf-recorder, xdg-desktop-portal-wlr | every copy fails | waiting frames fail |
| `wlr-output-management-unstable-v1` v4 | `output-management` | wlr-randr, kanshi, wdisplays; perspicax-settings, to list | every configuration fails | manager finished |
| `perspicax-shell-v1` (perspicax's own) | `shell` | perspicax-shell | told nothing; requests ignored | finished |
| `perspicax-settings-v1` (perspicax's own) | `settings` | perspicax-settings | nothing captured; a capture waiting is cancelled | a capture waiting is cancelled |

A rule a reload changes applies to the next client that looks, with no
global torn down, and what a client already holds is withdrawn as above.
`screencopy` needs the `capture` feature (part of `desktop`). A tool that asks
for the pointer (`grim -c`, OBS's *Show cursor*) gets it drawn in, and a
recording waiting for a change is sent one when the pointer moves. A display tool's
change to the monitors lasts for the session: it is put in force as the output
rules, exactly as if `[[output]]` had said it, and a reload that changes
`[[output]]` puts the file back in charge. Moving or rescaling a monitor
changes it where it stands, and its panels, wallpaper and maximized windows
follow; a new mode, or a monitor turned on or off, lights the monitors again.
A program that binds a monitor after it was named -- one plugged in, or lit
again -- is told then which workspace and windows are on it.

`[agent]` says what an agent may do on the seat beyond what the person grants
it: the programs it may start for itself (`launch`), and whether the session
serves it a socket (`socket`). Both are under [An agent inside the
session](#an-agent-inside-the-session). `[decorations.agent]` is how a window
it may act on is marked out.

Monitors are placed relative to each other, so the layout survives one being
unplugged: a monitor beside one that is missing goes to the right of the
rest, and windows left on a monitor that goes away come onto the nearest one
that remains. A `position` or `offset` is at most 65536 pixels from 0 either
way, which is further than any desk reaches; a display tool asking for one
further, or for a mode wider or taller than that, is refused. Only the outer edges of the desk flip and snap; between two
monitors the pointer passes through, and so does a window being moved unless
`[resistance] seams` holds it there.

A monitor's `scale` need not be a whole number: at 0.9 or 1.25 an
application lays itself out for the monitor's logical size. Each window is
told the scale of the monitor it is mostly on (`wp_fractional_scale_v1`), a
panel the scale of its own, and each is told again when the window moves or
the scale changes. A window draws its picture at that scale and says what
size it is shown at (`wp_viewporter`). So Firefox, GTK 4, Qt 6 and Chromium
are sharp and exactly the size of their windows, and the pointer lands on
what they drew. Below 1, a window is told no scale, or 1 if it was told one
before. Chromium told a scale below 1 shrinks its window by it but reads
the pointer as though it had not, so at 0.9 every click in Discord, VS Code
or Slack landed about a ninth further from the window's corner than what
the pointer was on, until Chromium's fix (crbug 438038776) reaches the
Electron each is built on. There Chromium, GTK and Qt draw at 1 and are
scaled down: the right size, the pointer on what they drew, a little soft.
Firefox takes its scale from the monitor's logical size and stays sharp. An
application that speaks neither draws at the next whole scale up and is
scaled to fit: the right size, a little soft. Xwayland alone is offered no
viewporter. With one, it would offer X games resolutions the monitor does
not have, scaled up to fill it, and neither where perspicax says an X
window is nor what the pointer finds would follow. X games keep the
monitor's own resolution.

None of that needs a second monitor to try. `--headless --size 2560x1440
--size 1920x1080` runs two virtual ones, each up to 65536 pixels a side,
which is what the live tests in
`crates/perspicax-compositor/tests/` do. On a seat, the kernel will light a
connector with nothing plugged into it on request (`echo on | sudo tee
/sys/class/drm/card1-HDMI-A-1/status`, and `detect` to undo it), and
perspicax treats it as a monitor nobody can see: the pointer crosses into it,
windows can be sent there and back, and unplugging it rescues them.

## Log in to it

A display manager (SDDM, GDM, LightDM, greetd) can offer Perspicax at its
greeter beside every other session. Install it as yourself. The script builds
first if it has to, and uses sudo only for the copy:

```sh
scripts/install.sh --features perspicax/desktop,perspicax-shell/full
```

It builds what `target/release` lacks, and builds again when the run names
features, by `--features` or `PERSPICAX_BUILD_FEATURES`. With the variable
exported, `scripts/install.sh` alone is enough, and with no build and nothing
named it builds the features above. A run that names none installs the build
that is there, so one made by hand is installed as it is. The settings app is
built apart, in a cargo run of its own (see [Build](#build)), and building it
leaves the session's programs as they are. As root it builds nothing: `sudo
scripts/install.sh` installs what you built.

That puts `perspicax`, `perspicax-shell` and `perspicax-settings` side by
side in `/usr/local/bin`, the session entry in
`/usr/local/share/wayland-sessions`, the settings app's entry in
`/usr/local/share/applications`, and the settings portal's two files under
`/usr/local/share/xdg-desktop-portal`. Some programs
read only from `/usr`: LightDM, greetd's greeters and older SDDM look for
sessions only in `/usr/share/wayland-sessions`, and xdg-desktop-portal for
portal files only in `/usr/share/xdg-desktop-portal/portals`. The script says
when either applies; `PREFIX=/usr scripts/install.sh` installs under `/usr`,
and `PORTALDIR` moves the portal file alone. `DESTDIR` and the other variables
`scripts/install.sh --help` lists are for a package.

Then, as the account that will log in, `perspicax doctor` says what the
session would lack. A session the display manager starts is given no flags,
so it serves an agent inside it a socket only when the config says `[agent]
socket = true`. Its `agent-socket` row says whether the config does.

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
  as well as its warnings; `RUST_LOG` changes that, as ever. What the
  programs the session starts print, the shell's warnings among it, is not
  in it but in the session's stderr, wherever the display manager keeps
  that: `~/.local/share/sddm/wayland-session.log` for SDDM.

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
once, with nothing to read and the session's stderr to print to. An entry of
yours replaces the system's of the same file name, and one of yours saying
`Hidden=true` turns the system's off. An entry is skipped when
it is hidden or disabled (`X-GNOME-Autostart-enabled=false`), when its
`OnlyShowIn` or `NotShowIn` rules out `perspicax`, or when its `TryExec`
program is not installed; one that needs a terminal is skipped with a line in
the log, and `RUST_LOG=info,perspicax_compositor=debug` names every entry
skipped and why. `xdg-autostart = false`, at the top of the config, turns the
folders off and keeps the list. A program both of them name starts twice:
take it out of one. An entry in the list written
`{ spawn = [...], agent = true }`, as a binding may be, starts a program an
agent may act on, until it exits; every other program the session starts is
the person's.

Another desktop on the same machine may never have started what those
folders hold -- Enlightenment starts only what its own startup list names --
so a first login here can start something that has never run there before:
PipeWire beside a PulseAudio that was the sound server until then, for one.
Turn off what you do not want with a `Hidden=true` entry of your own.

**The splash** is what a session shows from its first frame, in place of a
dark screen while all that comes up: the theme's `splash` colour, which is
`lock`'s unless written, with `[theme] logo` in the middle (see
[Themes](#themes)). It is drawn beneath everything, so a window that opens
early is seen over it, and it ends on each monitor as a wallpaper covers it:
the shell's, or any surface on the background layer over the whole monitor.
With nothing ever over it, as with `[shell] wallpaper = "none"` or no shell,
it ends after ten seconds. A save that turns it off ends it at once.

```toml
[splash]
enabled = false                # the plain grey, as before there was a splash
```

## The desktop

`perspicax-shell` is the desktop: a wallpaper, a panel, menus of the installed
applications, other programs' tray icons and the desktop folder's icons. It
is a program of its own and an ordinary Wayland client: its surfaces are
layer-shell surfaces, and its taskbar and pager speak the protocols waybar
speaks. A panel that crashes takes no window with it, nor leaves one frozen:
a window shown again by a switch, a restore, a tab or a monitor coming or
going draws at once, a panel running or not, and no window waits more than a
second to be told it may draw. Anything speaking layer-shell can stand in for
any piece of it.

perspicax starts it as the session starts, before `autostart`, and starts it
again if it crashes, or if it loses its connection to perspicax; the log says
which. A client that stops reading while input is sent to it is cut off once
a megabyte waits for it, half a minute or more of an ordinary mouse moving
over it, and the log says when it stopped. The shell warns of anything that
kept it from reading for a quarter of a second or more. A shell that refuses
its config waits for the file to be saved again. A save that changes
`[shell]` reaches it in place, without a restart. The tray needs a D-Bus
session bus, and a text-console login has none: start the session under one,
`dbus-run-session -- perspicax --seat`, or as `perspicax --session`, which
starts one itself. With `--mcp`, `perspicax --seat --mcp --own-bus` instead,
so that bus holds nothing of the agent's wire: see [What acting looks
like](#what-acting-looks-like).

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
| Taskbar | `panel` | each monitor's own windows; a right-click, a window's menu (with `menus`); resting on one, its whole title | — | the panel's; a window's menu as the root menu's; the tooltip, overlay, `perspicax-tooltip-<output>`, while the pointer rests | a `TabList` "Taskbar", the window in use selected, each `Tab`'s context menu its window's; a `Menu` named for the window, its ticks and dots `MenuItemCheckBox`es and `MenuItemRadio`s; nothing more for the tooltip |
| Pager | `panel` | the workspaces | — | the panel's | a `TabList` "Workspaces", the one showing selected |
| Keyboard layout | `panel` | before the tray, with two layouts or more | — | the panel's | a `Button` "Keyboard layout", the layout's name its value |
| Tray | `tray` (with `panel`, `menus`) | before the mixer; resting on an icon, its program's name for it | — | the panel's; its menus as the root menu's; its tooltip as a task's | a `Group` "Tray" of a `Button` for each icon, named by its tooltip |
| Mixer | `audio` (with `panel`, `menus`) | before the clock; the volume keys | the volume keys | the panel's; overlay, `perspicax-mixer-<output>`, while open; the volume shown, overlay, `perspicax-osd`, a moment | a `Group` "Sound" of a node for each piece; a `Dialog` "Sound"; a `Status` "Volume" |
| Clock | `panel` | the time, last | — | the panel's | a `Status` "Clock", the time its value |
| Pies | `pie` (with `menus`, `panel`) | a key or button bound to `{ pie = "<name>" }` | the same | overlay, `perspicax-pie-<output>`, while open | a `Menu` named for the pie of a `MenuItem` for each slot, the one pointed at selected |

A right-click on a task opens its window's menu beside it, as a tray icon's
opens beside the icon: Sticky, Always on top and Shade, ticked as the window
is; Send to workspace, the workspaces of the window's monitor with the one it
is on dotted, while the window has a workspace of its own and another to go
to; then Minimize or Restore, Maximize or Unmaximize, and Close. Each item
says what the window is to be, so a key that changed the window while the
menu was open does not turn the choice around. Shade is greyed out for a
window that draws its own frame, or is fullscreen. The shell asks
perspicax for the first four over `perspicax-shell-v1`, from version 5, and
the window for the rest over the taskbar's protocol, so a shell perspicax
tells nothing of its windows offers those alone. The menu does not yet open
from a titlebar ([#240](https://github.com/Awakened-Labs/Perspicax/issues/240)),
from an application's own titlebar
([#241](https://github.com/Awakened-Labs/Perspicax/issues/241)) or from a key
([#239](https://github.com/Awakened-Labs/Perspicax/issues/239)).

The pointer resting 0.7 s on a task or a status icon shows what it is in a
tooltip: the window's whole title, which a task showing its icon alone, or
its title cut short, does not; and the program's name for its icon, its
StatusNotifierItem tooltip's title, else its title. The tooltip stands just
off the bar, above a panel along the bottom and below one along the top,
centred on what it is for and kept on the monitor. Moving to another task or
icon moves it at once, and it follows what comes under a pointer that has
not moved: a window retitled, or the next task sliding along as one closes.
Leaving the bar, a press, or a menu, the mixer or a pie opening takes it
away, and what was pressed shows none until the pointer has been off it. It
takes no click and never the keyboard, and has no accessibility tree: what it
says is already the name of the `Tab` or `Button` it is for. While it shows,
an agent sees its surface, `perspicax-tooltip-<output>` on the overlay
layer, covering the strip beside the bar it stands on.
`[shell.panel] tooltips = false` turns tooltips off.

An agent reads the shell as it reads any application. On a seat it cannot
click it: like everything else the person's session starts, the shell gets no
agent consent. While a menu or a pie is open, its surface covers the whole
monitor, clear but for the menu, so to an agent everything under it is
covered until it closes. That includes the shell's own panel, which a person
can still click
([#146](https://github.com/Awakened-Labs/Perspicax/issues/146)). A narrow
panel's clear strip takes no clicks and still covers what is under it. A key
for a component the shell was built without is refused with the feature that
would provide it. Every key is optional:

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
lock = ["swaylock", "-f"]      # the start menu's Lock and `perspicax lock`,
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
tooltips = false               # no tooltip on resting on a task or a status
                               # icon; true, by default, shows what it is
items = ["start", "taskbar", "pager", "layout", "tray", "mixer", "clock"]
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
`["swayidle", "-w", "before-sleep", "perspicax lock -f"]` in `autostart` does.

Lock wears the theme. perspicax draws the lock screen itself, from the moment
it locks: `lock`'s colour, with `[theme] logo` on it, above where the ring will
be. swaylock, which `lock` runs unless it names another program, draws only
its ring and its text over that. It is given `-C /dev/null`, so it reads no
config file of its own, and then the theme as flags: no colour of its own for
the screen, so perspicax's shows through, its text in `lock-ink`, its ring in
`lock-ring`, `lock-accent` as keys are typed and `alert` for a wrong password
(see [Themes](#themes)), in `[theme] font`. The ring stays hidden until a key
is typed. Flags written in `lock` come after, so they win:
`lock = ["swaylock", "-f", "--ring-color", "00ff00"]` rings it green, and a
`--color` or `-i` of your own paints over perspicax's screen, logo and all. A
ring moved or enlarged with swaylock's own flags may cross the logo. To have
swaylock read a file of its own, name it, whole, since `~` is not expanded:
`lock = ["swaylock", "-C", "/home/me/.config/swaylock/config"]` runs as
written, and so does any other program, each drawing its own screen.

`perspicax lock` locks as Lock does, from a key or from swayidle: it runs
`lock` with its own arguments after `lock`'s, and becomes it. It reads the
config of the session it was started in, which the session tells every program
it starts in `PERSPICAX_CONFIG`; a config it cannot read still locks, in the
default theme. Run outside a perspicax session, where nothing of perspicax's is
drawn beneath it, it has swaylock paint the screen in `lock` itself.

```toml
[keys]
"Logo+l" = { spawn = ["perspicax", "lock"] }
```

A tray icon is any program's that registers one the StatusNotifierItem way,
as Qt, Electron and libappindicator programs do. The shell serves the
registry the programs look for, or shows what another program's lists if one
was there first. A left click activates the icon, a middle click activates it
the other way, and a right click opens its menu at the icon, as the start menu
opens at its button.

### The mixer

The panel's `mixer` item shows the sound, and opens a mixer beside it, as the
start button opens its menu. It speaks PulseAudio's protocol, which
PipeWire's `pipewire-pulse` speaks too, to the server `PULSE_SERVER` names,
or else the one in `$XDG_RUNTIME_DIR/pulse`. With none there it is crossed
out and disabled, and it comes back within ten seconds of one starting.

It is made of pieces, side by side, in the order `[shell.mixer] panel`
writes them:

- **`mute`.** A speaker, with as many waves as the sound is loud, crossed out
  while it is muted. A click mutes the sound, or brings it back.
- **`volume`.** How loud the sound is, in percent.
- **`output`.** What the sound plays from: a glyph of its kind and a short
  name, the one you gave it, or the monitor's on HDMI, or an attached
  device's own, or the jack's: "Speakers", "E8", "DELL U2720Q". A click moves
  the sound to the next output, past those you hid.
- **`levels`.** Bars moving with what plays, as winamp's did, the deep notes
  at the left. Still, and costing nothing, when nothing plays.
- **`mic`.** A microphone, crossed out while it is muted. A click mutes it,
  and the wheel and the middle button over it are the microphone's.
- **`open`.** Three sliders. A click opens the mixer.

On any piece the right button opens the mixer, as a click on `volume` or
`levels` does. The wheel moves the volume to the next multiple of `step`, no
louder than `limit`, and the middle button mutes it. With no table, the item
is a speaker, the output and the mixer's button:

```toml
[shell.panel]
items = ["start", "taskbar", "pager", "layout", "tray", "mixer", "clock"]

[shell.mixer]
panel = ["mute", "levels", "output", "open"]   # in order; by default
                               # ["mute", "output", "open"]
step = 5                       # percent a notch of the wheel or a volume key
                               # moves the volume, 1 to 25
limit = 100                    # the loudest a notch or a key takes it, 1 to
                               # 150; one set louder elsewhere is left there
osd = true                     # show the volume a moment when it changes
```

The mixer has, for the output, a button that mutes it and a slider, and
under them every output, the one the sound is on marked, each by its whole
name: its card, its jack, and the monitor plugged into it or how it is
attached, "Built-in Audio — Speakers", "E8 — Headphones · Bluetooth", "TU104
HD Audio Controller — HDMI / DisplayPort 2 · DELL U2720Q". A click on one
moves the sound there, switching its card's profile first if it must. A jack with nothing plugged
into it is folded under "Not connected". The inputs follow, the same way.
Then, while programs play, "Applications": each program's icon and name, a
button that mutes it and a slider of its own, and the output it plays on; a
click on its row lists the outputs it could, and a click on one moves that
program alone. While the mixer is open, each output that plays has a meter
along its row. Tab and the arrows move through it, left and right step a
slider, Page Up and Page Down move it ten, Home and End take it to silence
or to `limit`, Space and Return press, and Escape closes it, as a click off
it does.

The three dots at a row's end unfold what tells that device apart: where it
is attached, its chip, its ALSA card or its Bluetooth address, its card's
profile and its key. Under them, a field to name it: a name kept with Return
is what the mixer, the `output` piece and the volume shown call it from then
on, and a blank one gives it its own back. Test sound plays a short chime on
that output alone, wherever the sound is; on one that would need its card's
profile or jack switched, it asks you to choose it first. Hide folds the
device under "Hidden", and the `output` piece never moves to it; hiding the
one the sound is on leaves the sound there. "Hide all of" its card hides each
of the card's jacks, those it gains later too: the four HDMI outputs of a
graphics card go at one click, and Unhide, under "Hidden", brings them back.
An input is metered while its details are open, if ALSA reads it, and so
never a Bluetooth headset's: recording one switches the headset to a call's
poorer sound.

What you hid and named is yours, not the config's: the shell keeps it in
`$XDG_STATE_HOME/perspicax/mixer.json` (`~/.local/state` without it),
readable by you alone, by each device's key, a jack's `<card>/<port>`
(`alsa_card.pci-0000_01_00.1/hdmi-output-1`) or a card's name. A Bluetooth
card's name is made of its address, so a headset keeps its name across
reconnecting. A file that cannot be read is warned of in the log and started
afresh.

With `osd` on, a change to the output's volume, its muting or which output it
is, made anywhere but the panel and the mixer, a volume key or `wpctl` or
another mixer, shows the volume for a second and a half, above the panel of
the monitor the pointer is on: the speaker, the output's name, the percent
and a bar. It takes
no clicks. The sound is followed for it even with no mixer on a panel, so
`minimal`, which has none, shows its volume keys working.

Both profiles bind the volume keys: XF86AudioRaiseVolume and
XF86AudioLowerVolume to `volume-up` and `volume-down`, which step as the
wheel does, XF86AudioMute to `volume-mute` and XF86AudioMicMute to
`mic-mute`. A binding of your own for one of the keys wins. perspicax holds
no volume of its own: it tells the shell, which asks the sound server, so at
the lock screen, where the shell is told nothing, they do nothing. With
`[shell] enabled = false` the keys are left to the programs, and a binding
to one of the four is refused. A shell built without `audio` refuses
`"mixer"` in `items` and the `[shell.mixer]` table, but for `osd = false`
alone.

### Pies

A pie is PieDock's: a ring of icons that opens centred on the pointer when a
key or a button bound to `{ pie = "<name>" }` is pressed, and chooses by
direction. Pointing anywhere past its middle, out to the screen's edge,
picks the icon that way, which grows as the pointer turns toward it and has
its name in the middle. Its neighbours grow too, less the further round from
the pointer they are, as `grow` and `grow-falloff` say. The icons stay where
they are, so a grown one is drawn over its neighbours. Then:

- **Left button.** It opens a submenu in the pie's place, or starts the
  program. If the program has windows open, it brings the next of them
  forward instead, so pressing again goes round them.
- **Middle button.** It starts the program, whatever is open.
- **Right button.** It goes back out of a submenu, and closes the pie from
  its first ring.
- **Wheel.** It spins the pie a place a notch. What is picked changes at
  once, and the ring glides round to it.
- **Keys.** Escape closes it, Enter is the left button, Up and Down spin
  it, and Backspace is the right button.
- **Its own binding.** Pressed again, it closes the pie.

```toml
[mouse.anywhere]
"Mouse8" = { pie = "launchers" }    # the thumb button, taken from every window

[shell.pie]
size = 512                          # across, in pixels; 128 to 1024
grow = 2.0                          # the icon pointed at, times its size; 1 to 3
grow-falloff = 90                   # degrees round from the pointer that icons
                                    # still grow, less in a line; 1 to 180
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
motion = "reduced"             # every animation at once; normal unset
logo = "~/Pictures/logo.png"   # on the lock screen and the splash: perspicax
                               # (the default), a PNG's path, or none

[theme.palette]                # any role below, written over the theme's own
accent = "#e93d5a"
panel = "#102030"
```

`perspicax` is what was drawn before there were themes: blue titlebars, a dark
panel, light menus. The two Breezes take their window, view, header and
selection colours from Plasma's Breeze schemes. The lock screen is dark,
`#0c2440`, but under `breeze-light`, which locks on `#cbd8df`; its ring is
terracotta under all three.

The palette is a set of roles, not of widgets, so one colour written recolours
everything that has that job. A colour is `"#rrggbb"`. Only `selected` and
`label-shadow` may be see-through, as `"#rrggbbaa"`: an opaque panel or menu
is what lets the compositor prove to an agent what it covers (a see-through
one covers all the same, on policy). Writing one
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
| `lock` | the lock screen, behind the ring a password is typed into | — | `#0c2440` |
| `lock-ink` | the lock screen's text | its ink | `#ffffff` |
| `lock-ring` | the lock screen's ring, at rest: the brand's terracotta on any screen | — | `#c06a3c` |
| `lock-accent` | the lock screen's ring as keys are typed, and while a password is checked | `accent` | `#3daee9` |
| `alert` | a refusal: a wrong password, and a key taken back while typing one | — | `#da4453` |
| `title-close-hover` | a titlebar's close button under the pointer | `alert` | `#da4453` |
| `splash` | the splash a session shows as it starts, behind the logo | `lock` | `#0c2440` |

`logo` is what the lock screen and the splash show over their colour.
`perspicax`, the default, is Perspicax's mark over its name, lettered in white
on a dark colour and in navy on a light one, so it reads on whatever `lock` and
`splash` are. A path names a PNG of your own, found as a wallpaper is (`~` is
your home, and a relative path is beside the config), and shown on every colour
as it is, never larger than its own pixels. One that cannot be read, is not a
PNG, or is more than 4096 pixels a side is said once in the log, and
Perspicax's is shown instead until the config is saved again, when it is read
afresh. `none` leaves the colour alone.

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
  window. A client's own `_NET_WM_WINDOW_OPACITY` is ignored. One never takes
  the keyboard, so the window it was opened from stays in use, and undimmed,
  while it is open.

**For your eyes, not an agent's.** A see-through window still covers what is
behind it as far as an agent is told: a node beneath it is `occluded`, naming
it, exactly as if it were opaque. You chose to look through it, and an agent
is not expected to read what shows there. A picture of the monitor shows what
you see, the text faintly behind the window included, and `window_list` gives
such a window an `opacity`, so an agent shown that text and told it is
covered has the reason. A picture of the window by itself shows it whole.
What an application draws see-through itself, such as a terminal's
translucent background, rounded corners, or the shadow around its window,
covers what is behind it in the same way: nothing an application says can
tell a shadow from an opaque band. The shadow perspicax casts under a window
it frames is not the application's, and covers nothing.

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
well. classic binds Logo+Space, so applications no longer get it. A program
that wants every key, a VM viewer or a remote desktop, has no way yet to ask
for the ones a binding takes (#214). A reload
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

`minimal` binds two of its own, as Openbox does: over a titlebar, `WheelUp`
rolls the window up to it and `WheelDown` rolls it down again, and a second
notch the same way does nothing more. `classic` binds none. A table of your
own goes over the profile's, so `"none"` hands either back, and a binding
replaces it:

```toml
[mouse.titlebar]
"WheelUp" = "none"                # minimal's rolling up, handed back
"Double+Mouse1" = "toggle-shade"  # roll up, as Fluxbox does, not maximize
```

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
straight to the window or surface the agent named. Nor does an agent act while
a button is down: a move, a resize or a selection held still is still yours,
and the agent waits until you let go.

Locking the screen takes the pointer as well as the keyboard, at once and with
the mouse at rest. A click, a scroll, or a tap of the touchpad to wake the
screen lands on the lock screen, never in the window it hides, and swaylock
hides the cursor without a nudge, on whichever monitor the pointer is. What
your hand held lets go as the screen locks: an open menu closes, as a click
outside it would, so it cannot take the password; a window being moved or
resized stays where it got to; a tab being dragged goes back to its group. No
window can open a menu over the lock screen, nor take the keyboard from it: a
window that opens behind it, asks to be brought forward, or comes forward as
the tab in front of it closes gets none of what you type, so a chat window, a
terminal or a dialog that pops up while you type your password never sees it.
Unlocked, the window under the pointer has the pointer back, and its own cursor
with it, and the window on top has the keyboard, often the one that opened
meanwhile.

The program that locked the screen holds the lock, and only it can unlock it.
Another program that asks to lock while it runs is refused, and cannot unlock
the screen either: a second swaylock, as swayidle may start, says another lock
screen is running and exits, and a program that locks only to unlock straight
away is cut off. If the program holding the lock dies without unlocking, the
screen stays locked, and none of your windows show. Start a lock program again
-- `perspicax lock` from another virtual terminal or over ssh, with the
session's `WAYLAND_DISPLAY` set -- and it takes the lock over, and unlocks as
usual.

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

## The settings app

`perspicax-settings` puts every setting on a page, and writes it into the
same `config.toml` an editor writes, in place, so the two can be used turn
and turn about: a line added in vi is on its page as soon as the app's
window has the keyboard again, and a change made in the app is in the file
for vi to find. Start it from the start menu or a launcher, as Perspicax
Settings, or as `perspicax-settings`. It writes the session's own file, which
the session tells every program it starts in `PERSPICAX_CONFIG`, or else
`$XDG_CONFIG_HOME/perspicax/config.toml`; `--config PATH` names another.

**The file stays yours.** A change rewrites the lines it must and no others:
every comment, blank line, alignment and order is kept. A key the file
writes is changed where it stands, and keeps the comment beside it. One it
does not write fills the file's own commented-out example, `# border = 2`,
where it stands, as a table fills its commented-out header, `# [splash]`; a
table the file has no example of goes at the end. A binding is found by the
chord it means, not by how it is written, so binding Logo+a again changes a
`"Super+a"` line and keeps that spelling. A value already in force is not
written, so the file goes on following its profile there, and a key's
"Reset to the profile's" takes its line out.

**Only a file the session takes is written.** A change is made in a file
beside the config and checked by the installed perspicax, with `perspicax
config check`, as both programs a session starts would read it. Only then is
it renamed over the config, so the session, which reads the file again when
it is saved, only ever reads a whole one. A change either program refuses is
not written: the window says why, in that program's words, and offers each
fix that would be taken. Switching to `minimal` with `[shell.panel]` keys
written, which a profile with no panel refuses, offers to take them out or
to turn the panel on. A file the session already refuses says so above every
page, with its fixes, and one that is not TOML at all is never written, and
can be opened in an editor from there. With no `perspicax` beside the app or
on `PATH`, nothing is written. If another program wrote the file since the
app read it, the change is made again on top of what it wrote.

Its pages, down the left of the window:

- **Keys:** every chord the session binds and every one the file writes,
  each with whose it is: the profile's, yours, yours over the profile's, or
  unbound by you.
- **Session, Focus, Workspaces, Windows, Keyboard, Pointer, Shell, Panel,
  Theme and Decorations:** every other setting that holds one value, each
  with whose it is.
- **Outputs:** each monitor's `[[output]]`.
- **Protocols:** `[protocols]` and `[agent]`, to read.

**A chord is bound by pressing it.** "Add a binding…", or a row's "Move to
another chord…", asks perspicax for the next chord pressed. perspicax tells
the app from where it matches bindings, before any program sees the key, so
pressing Logo+Return starts no terminal, and the chord is written as the
session will match it: under `"us,ru"` with Russian in use, Logo+й is
written `Logo+q` (see [Keyboards](#keyboards)). Tapping Logo binds the tap,
and Escape on its own gives up. Ctrl+Alt+Backspace and Ctrl+Alt+F1…F12 still
do what they do, and the page says nothing can bind them. A key no config
can name yet, the keypad's Enter for one, is said to be, for another to be
pressed. The wait ends if the window loses the keyboard or the screen locks.
Only a program `[protocols] settings` admits is told a chord, by default
`perspicax-settings`, by name. Left out, or outside a perspicax session, the
app has the chord typed instead, and reads it as the config reads it, with
the reason under it when it does not read. The chord is then
given an action, a pie, or a program to start, with "An agent may act on
what it starts" for `agent = true`. A row can also be given another action,
unbound, or reset to the profile's.

**Every other page writes as it is changed.** A switch or a choice is
written when it is clicked, and a number, a colour or a word when its field
loses the keyboard, not as it is typed; Escape puts the field back. A value
out of its range is refused beside its field, in the parser's words. A
colour is typed, `#rrggbb`, beside a swatch of it; a picker that writes once,
as it closes, is to come (#216). The pages read the file as if perspicax
were built with everything, so `xwayland` and the splash have their switches
even where the installed perspicax lacks them, and a change to one is
refused in its words. A section whose switch is off, the shell or the panel,
shows only the switch, and the file keeps what it writes for the rest.

What has no page is left to the file, and an editor: what names things of
your own, `[opacity.apps]`, `[placement] forget` and `[shell.wallpapers]`;
`[shell.pie]`, `[shell.start-menu]` and `[shell.mixer]`; `[mouse]` and
`autostart`; and `[decorations] focused` and `unfocused`, which the Theme
page writes in their newer place, as the palette's `title-focused` and
`title-unfocused`. Writing both places is refused, and the fix takes the
older one out.

**Keyboard layouts** are picked from xkb's own list, the one `setxkbmap`
reads: `rules/evdev.lst`, or `rules/<rules>.lst` for the `rules` written,
under `$XKB_CONFIG_ROOT`, or else `/usr/share/xkeyboard-config-2`, or else
`/usr/share/X11/xkb`. Each
is shown by its name and xkb's description, `us — English (US)`, with its
variant, and `layout` and `variant` are written together. The list leaves
out xkb's extra layouts, and any of your own in `~/.config/xkb`, so a layout
the file writes that the list lacks is kept, and said to be. With no list,
the two are typed.

**Outputs** has each monitor connected, with its modes, as the session lists
them to a display tool, then each rule the file writes for one that is not.
A monitor can be turned off, given one of its modes (a size, then a refresh
rate, or the preferred or the fastest), a scale, and a place: right of the
others, at a position, or beside another, with an offset. A preview draws
the arrangement; dragging monitors in it is to come (#217). Nothing is
written until Apply. Then the monitors change, and the window asks "Keep
these settings? Reverting in 15 s." Keep keeps them. "Revert now", the 15
seconds running out, or closing the window writes back the values the change
altered, over whatever else the file says by then. If the app is killed in
that time, nothing is put back: the change stays written. A change that
would leave no connected monitor on is refused, and nothing is written. The
monitors are listed over `wlr-output-management`, which the default
`[protocols] output-management` admits `perspicax-settings` to; with a rule
that leaves it out, the page has the file's rules alone, each mode typed:
`2560x1440@144`.

**Protocols** shows each `[protocols]` rule and `[agent] launch` in force,
beside the profile's, with `[agent] socket`, and changes none of them. Who
may reach past their own windows, and what an agent may do, is the file's to
say, in an editor: the app never writes it, so nothing that drives the app
can widen it. "Open the file" opens it.

**No agent drives it.** The file it writes is what holds an agent back, so
an agent that could drive the app could grant itself anything. perspicax
never lets an agent act on its windows, whatever was granted, and the config
refuses `agent = true` on a binding or an `autostart` entry that starts it,
and `perspicax-settings` in `[agent] launch`. It is known by the name its
program runs as, wherever it is installed.

## Test

Gates, as CI runs them, in four jobs side by side:

```sh
# check: the workspace, which needs only xkbcommon's development package
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked -p perspicax-shell --all-targets -- -D warnings   # no components
cargo test --locked --workspace

# seat: the daily-driver build, with the seat's development packages (see Build)
cargo clippy --locked --workspace --all-targets --features perspicax/desktop -- -D warnings
cargo clippy --locked --workspace --all-targets --features perspicax/capture -- -D warnings
cargo clippy --locked --workspace --all-targets --features perspicax/seat -- -D warnings
cargo test --locked --workspace --features perspicax/desktop

# settings: the settings app's window, in a cargo run of its own
cargo clippy --locked -p perspicax-settings --all-targets --features gui -- -D warnings
cargo test --locked -p perspicax-settings --features gui

# deny
cargo deny --all-features check
```

The settings job drives the app's pages headless, finding each control by
what a screen reader hears, with no compositor. Tests that need a real
accessibility bus, real applications or a Wayland socket are `#[ignore]`d, so
every gate above stays green on a machine with no graphical session. To run
them:

```sh
export XDG_RUNTIME_DIR=/run/user/$(id -u)
export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
cargo test -p perspicax-atspi --test live -- --ignored --test-threads=1
cargo test -p perspicax --test act  -- --ignored --test-threads=1
cargo test -p perspicax-compositor --all-features -- --ignored --test-threads=1
```

The second is v1's exit criterion, and it is the one command that asserts the
whole claim: two toolkits hosted, a control clicked in each with a receipt to
show for it, and a covered control refused with the occluding surface named.
The third is the compositor's own: headless, on virtual monitors, with
Wayland clients of its own.

Without those two variables an SSH session finds an empty desktop and reports
no error worth reading. `perspicax-probe apps` says what the bus can actually see.

`scripts/provision-testbed.sh` builds that machine from a fresh Debian 13
install — the desktop, both toolkits, the bus and the pinned toolchain — for
anyone reproducing the figures above rather than taking them on trust.
`ci/live-tests.sh` is the other half: the same suite with no desktop at all.

## Licence

Apache-2.0. See `LICENSE`.
