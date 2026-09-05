#!/usr/bin/env bash
#
# The whole test suite, live tests included, inside a session that actually has
# an accessibility bus and two real toolkit applications on it.
#
# Invoked as:  xvfb-run -a dbus-run-session -- ci/live-tests.sh
#
# Both wrappers are load-bearing and neither is interchangeable with the other:
# `dbus-run-session` supplies a private session bus, from which `org.a11y.Bus`
# and then `at-spi2-registryd` start themselves by D-Bus activation; `xvfb-run`
# supplies the X display without which neither toolkit will map a window, and
# an application with no window has no accessible tree to read.
#
# Verified end to end on Debian 13 before being written down. From M2 this also
# runs the demo the milestone is judged on -- a compositor of our own hosting
# both toolkits -- which needs XDG_RUNTIME_DIR for its Wayland socket, a thing
# a CI container does not have and a login session does.
#
# From M3 it runs `crates/wm/tests/act.rs` as well, which is v1's exit
# criterion: a control clicked in each toolkit with a receipt for it, and a
# covered control refused with the occluding surface named. It needs nothing
# this script did not already arrange -- the applications it drives are ones
# `wm` spawns itself, and `cargo test --include-ignored` below picks the tests
# up without being told about them. The two applications started further down
# are for `wm-atspi`'s live tests, and `observe` skips them because their
# processes own none of our surfaces.

set -euo pipefail

# The gallery's path is architecture-qualified, so ask dpkg rather than
# hard-coding a triplet that is right on exactly one runner. Exported so the
# M2 demo test finds the same one rather than repeating the search.
gallery=$(dpkg -L qt6-base-examples | grep -m1 '/widgets/gallery/bin/gallery$')
export WM_QT_GALLERY="$gallery"

# Where a Wayland socket can be bound. A login session has one; a container
# does not, and the failure is `wm` refusing to start with no obvious cause.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/wm-runtime}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# Turn accessibility on for this session, and check that it took.
#
# `org.a11y.Status.IsEnabled` is what Qt's AT-SPI bridge gates on: with it
# false the gallery starts perfectly, draws its window, prints nothing, and
# never joins the accessibility bus at all. GTK's ATK bridge ignores the flag
# and registers either way, so the symptom is one toolkit silently missing.
#
# A desktop sets it from dconf (`org.gnome.desktop.interface
# toolkit-accessibility`), which is exactly why this was invisible on a
# workstation and fatal in a container: a fresh HOME has no dconf state, so the
# flag defaults to false.
echo "enabling accessibility for this session"
gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Set \
    org.a11y.Status IsEnabled "<true>" >/dev/null

enabled=$(gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
    --method org.freedesktop.DBus.Properties.Get org.a11y.Status IsEnabled)
if [[ "$enabled" != *true* ]]; then
    echo "could not enable accessibility: IsEnabled is $enabled" >&2
    exit 1
fi

echo "starting the toolkit applications"
gtk4-widget-factory >/tmp/gtk4-widget-factory.log 2>&1 &
# Qt compiles its AT-SPI bridge into QtGui under two feature flags rather than
# shipping it as a plugin, and asks for this variable before using it.
QT_ACCESSIBILITY=1 "$gallery" >/tmp/qt6-gallery.log 2>&1 &

# `--all-targets`, not `--tests`. `--tests` builds test harnesses and does NOT
# produce `target/debug/wm-probe`, so the poll below would run a binary that
# does not exist -- which is exactly what happened the first time this ran in a
# container, while passing on a workstation where an earlier build had left one
# behind.
cargo build --locked --workspace --all-targets

probe=./target/debug/wm-probe
if [ ! -x "$probe" ]; then
    echo "wm-probe was not built at $probe -- check the build target selection" >&2
    exit 1
fi

# Poll rather than sleep. A fixed wait is either too short on a loaded runner
# or wasted on an idle one, and when it is too short the failure surfaces as a
# confusing test error rather than as "the application never arrived".
#
# stderr is captured rather than discarded. Throwing it away is how a missing
# binary came to be reported as an empty accessibility bus.
echo "waiting for both applications to reach the accessibility bus"
on_bus=""
for _ in $(seq 60); do
    on_bus=$("$probe" apps 2>&1 || true)
    if grep -q gtk4-widget-factory <<<"$on_bus" && grep -q gallery <<<"$on_bus"; then
        echo "$on_bus"
        break
    fi
    sleep 1
done

if ! grep -q gtk4-widget-factory <<<"$on_bus" || ! grep -q gallery <<<"$on_bus"; then
    echo "an application never reached the accessibility bus." >&2
    echo "--- wm-probe apps said ---" >&2; echo "${on_bus:-<no output at all>}" >&2
    echo "--- gtk4-widget-factory ---" >&2; cat /tmp/gtk4-widget-factory.log >&2 || true
    echo "--- qt6 gallery ---" >&2;        cat /tmp/qt6-gallery.log >&2 || true
    exit 1
fi

# One thread: the live tests read and poke the same two applications, and
# interleaving them would make each one's result depend on the others.
cargo test --locked --workspace -- --include-ignored --test-threads=1
