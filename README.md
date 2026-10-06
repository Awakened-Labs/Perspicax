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
perspicax-mcp          MCP server (rmcp, stdio) — eight tools, DTOs, receipts  [portable]
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
| **W6** | Polish: themes, keymaps, a session entry for display managers | |

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
an error naming what to send first, and the server waits on. Eight
tools: `window_list`, `observe`, `resolve`, `act`, `window_close`,
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
agent sees an occlusion before it spends a call discovering one. A window on a
workspace that is not showing is refused as `other_workspace`, naming the
workspace, a window that is a tab behind another as `inactive_tab`, naming the
tab in front, and a node hanging off the edge of every monitor as `off_screen`.
None is cleared by the agent switching the person's screen on its own.

**A titlebar is pixels no client drew.** The frame perspicax draws around a
window goes into the facts beside the window, so a node of another window
under a titlebar is refused as `occluded`, naming the window the titlebar
belongs to, and the verdict is proof rather than policy: the frame is drawn
solid. A window's own frame sits outside it and never covers its own nodes.
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
keyboard or pointer: an act in the middle of their typing would race it.

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
`--spawn` programs and the autostart list start once Xwayland is ready. An
X11 window's origin says so: the X client's pid comes from the X server's
X-Resource answer rather than the kernel, and every X client shares one consent
decision, because X11 lets them read and drive each other.

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
scrolling over the desktop. It does not snap. In both, a fullscreen window
covers the panels while it is the one in use, and goes back under them when
another window or a menu takes the keyboard. Every key below is optional and
overrides the profile one setting at a time. A misspelled key, or a key for a
feature this build left out, is refused with its name rather than ignored.

```toml
profile = "minimal"            # or "classic"; minimal focuses under the pointer

[focus]
model = "sloppy"               # click | sloppy | strict
autoraise = false

[keys]
"Logo+Return" = { spawn = ["foot"] }
"Logo+d" = { spawn = ["fuzzel"] }
"Alt+F4" = "none"              # hand a profile's chord back to the client
"Alt+F1" = "root-menu"         # or "start-menu": the desktop shell's menus
"Logo" = "none"                # Logo alone is a tap: pressed, let go, nothing
                               # in between; no other modifier can be tapped

drag = "Logo"                  # the drag modifier; "none" turns drags off

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
scroll = true                  # scroll over the desktop

[snap]
drag = true
threshold = 4                  # pixels from the edge

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

autostart = [["mako"], ["nm-applet", "--indicator"]]

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
key asks for a menu and may end the session. Listing windows and workspaces
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
`screencopy` needs the `capture` feature (part of `desktop`). A display tool's
change to the monitors lasts for the session: it is put in force as the output
rules, exactly as if `[[output]]` had said it, and a reload that changes
`[[output]]` puts the file back in charge. Moving a monitor just moves it; a
new mode, scale, or a monitor turned on or off lights the monitors again.

Monitors are placed relative to each other, so the layout survives one being
unplugged: a monitor beside one that is missing goes to the right of the
rest, and windows left on a monitor that goes away come onto the nearest one
that remains. Only the outer edges of the desk flip and snap; between two
monitors the pointer passes through.

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
  bus ends with the session, and every service it started ends with it.
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
the log names the keyring that is in the way.

Each component is a cargo feature of `perspicax-shell`, and `full` is all of
them; none is on by default. The profiles turn on what the build has:

| Component | Feature | `classic` | `minimal` | Surface: layer, namespace | What an agent reads |
|---|---|---|---|---|---|
| Wallpaper | `wallpaper` | Plasma's blue | a dark grey | background, `perspicax-desktop-<output>` | a `Window` named for its surface |
| Desktop icons | `icons` (with `wallpaper`, `menus`) | the first monitor | off | drawn on the wallpaper's | a `List` "Desktop" of a `ListItem` for each icon, the selected one selected |
| Root menu | `menus` | right-click on the wallpaper, or `root-menu`'s key | the same | overlay, `perspicax-menu-<output>`, while open | a `Menu` "Root menu" of `MenuItem`s |
| Panel | `panel` | along the bottom of every monitor | none | top, `perspicax-panel-<output>` | a `Toolbar` "Panel" holding what follows |
| Start button and menu | `menus` | the panel's first item; a tap of Logo | — | the panel's; its menu as the root menu's | a `Button` "Start"; a `Menu` "Start menu" with a search line |
| Taskbar | `panel` | each monitor's own windows | — | the panel's | a `TabList` "Taskbar", the window in use selected |
| Pager | `panel` | the workspaces | — | the panel's | a `TabList` "Workspaces", the one showing selected |
| Tray | `tray` (with `panel`, `menus`) | before the clock | — | the panel's; its menus as the root menu's | a `Group` "Tray" of a `Button` for each icon, named by its tooltip |
| Clock | `panel` | the time, last | — | the panel's | a `Status` "Clock", the time its value |

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
lock = ["swaylock", "-f"]      # the start menu's Lock

[shell.wallpapers]             # a workspace's own, by number from 1, row by row
2 = "#2d5a4f"
3 = { wallpaper = "~/Pictures/tile.png", mode = "tile" }

[shell.panel]
enabled = true                 # false: no panel
edge = "top"                   # or "bottom"
height = 32
outputs = "first"              # "all", or a list of connectors: ["DP-1"]
taskbar = "all"                # every window on every panel; "this-output"
                               # lists each monitor's own
items = ["start", "taskbar", "pager", "tray", "clock"]
clock = "%a %e %b %H:%M"       # as strftime writes it
```

The menu file is TOML too: `mode = "extend"` puts its items above the
applications and `"replace"` puts them instead, and each `[[items]]` is one of
`exec = [...]` with a `label`, `app = "firefox"` for an installed
application, `separator = true`, a submenu with `items = [...]`,
`applications = true` for the applications by group, or `session = true` for
Lock and Log Out.

A tray icon is any program's that registers one the StatusNotifierItem way,
as Qt, Electron and libappindicator programs do. The shell serves the
registry the programs look for, or shows what another program's lists if one
was there first. A left click activates the icon, a middle click activates it
the other way, and a right click opens its menu at the icon, as the start menu
opens at its button.

**Coming from W4.** `classic` now starts the shell, so a config that starts
waybar and swaybg from `autostart` gets two panels and two wallpapers. Take
them out of `autostart`, or keep them and set `[shell] enabled = false`.

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
