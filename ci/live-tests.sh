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
# Verified end to end on Debian 13 before being written down: 87 tests, of
# which 11 need the bus.

set -euo pipefail

# The gallery's path is architecture-qualified, so ask dpkg rather than
# hard-coding a triplet that is right on exactly one runner.
gallery=$(dpkg -L qt6-base-examples | grep -m1 '/widgets/gallery/bin/gallery$')

echo "starting the toolkit applications"
gtk4-widget-factory >/tmp/gtk4-widget-factory.log 2>&1 &
# Qt compiles its AT-SPI bridge into QtGui under two feature flags rather than
# shipping it as a plugin, and asks for this variable before using it.
QT_ACCESSIBILITY=1 "$gallery" >/tmp/qt6-gallery.log 2>&1 &

cargo build --workspace --tests

# Poll rather than sleep. A fixed wait is either too short on a loaded runner
# or wasted on an idle one, and when it is too short the failure surfaces as a
# confusing test error rather than as "the application never arrived".
echo "waiting for both applications to reach the accessibility bus"
for _ in $(seq 60); do
    on_bus=$(./target/debug/wm-probe apps 2>/dev/null || true)
    if grep -q gtk4-widget-factory <<<"$on_bus" && grep -q gallery <<<"$on_bus"; then
        echo "$on_bus"
        break
    fi
    sleep 1
done

if ! grep -q gtk4-widget-factory <<<"${on_bus:-}" || ! grep -q gallery <<<"${on_bus:-}"; then
    echo "an application never reached the accessibility bus:" >&2
    echo "${on_bus:-<nothing on the bus>}" >&2
    echo "--- gtk4-widget-factory ---" >&2; cat /tmp/gtk4-widget-factory.log >&2 || true
    echo "--- qt6 gallery ---" >&2;        cat /tmp/qt6-gallery.log >&2 || true
    exit 1
fi

# One thread: the live tests read and poke the same two applications, and
# interleaving them would make each one's result depend on the others.
cargo test --workspace -- --include-ignored --test-threads=1
