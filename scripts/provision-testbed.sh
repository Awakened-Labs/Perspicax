#!/usr/bin/env bash
#
# Build the box the README's numbers were measured on, from a fresh Debian 13
# install. Idempotent.
#
# `ci/live-tests.sh` builds a *session* inside a container that already has the
# packages. This builds the *machine*: a real desktop on a real seat, which is
# the thing a container cannot be and which several of this project's claims
# are measured against. The two are deliberately not merged -- CI proves the
# suite runs headless, this proves it runs where a person can watch it.
#
# Verified 2026-09-04 on a Proxmox VM: Debian 13.6, virtio-gpu, 8 cores, 16 GB.

set -euo pipefail

WHO="${SUDO_USER:-$USER}"
say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

say "packages"
sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq

# GNOME, and specifically GNOME, because the accessibility stack this project
# reads through IS GNOME's -- at-spi2-core, its bus launcher and its registry.
# The README's per-toolkit timings and its Xorg-vs-Wayland coordinate findings
# were all taken here, so a different desktop is a different measurement rather
# than the same one on other hardware. `gnome-session-xsession` is what makes
# the Xorg session selectable at all: without it the box can only offer
# Wayland, and half of the `CoordType::Screen` comparison becomes unrunnable.
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
    gnome-core gdm3 gnome-session-xsession

# The two toolkits we claim to reach, plus the bus they speak over.
#
# `qt6-wayland` is easy to miss and silent when missing: without it the Qt
# gallery has no Wayland platform plugin, falls back to xcb, and connects to
# whatever X server is around instead of to our compositor -- which reads as a
# broken join rather than as a missing package.
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
    at-spi2-core gtk-4-examples qt6-base-examples qt6-wayland \
    python3-pyatspi

# Build dependencies. `libxkbcommon-dev` is the ONLY system library this
# workspace needs, which is worth stating because it is counterintuitive for a
# compositor: `wm-compositor` takes Smithay with `default-features = false` and
# no renderer, so there is no EGL, no GL, no GBM, no DRM and no libinput in the
# graph -- and no libwayland either, since wayland-backend's Rust server
# implementation is what `use_system_lib` would have replaced. Smithay links
# libxkbcommon unconditionally for keymaps, and that is the whole list.
#
# An earlier version of this script installed the whole DRM/GBM/EGL/libinput
# set on the assumption a compositor must need it. It does not, and installing
# them hides the fact that this tree builds on a box with no GPU at all.
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
    build-essential pkg-config curl git rsync ca-certificates libxkbcommon-dev

# Headless path, so the CI shape can be reproduced here without a container.
# `xauth` is a Recommends of xvfb rather than a dependency, and `xvfb-run` exits
# 3 with "xauth command not found" without it. `dbus-run-session` lives in
# `dbus-daemon` on Debian 13 -- NOT in `dbus-x11`, which is where a Qt5-era
# memory says to look.
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y xvfb xauth dbus-daemon

say "rust toolchain"
# Debian ships rustc 1.85; this workspace declares rust-version 1.92 with
# edition 2024, so the distro toolchain cannot build the tree at all.
#
# 1.93 is the default specifically because `.gitlab-ci.yml` pins
# `rust:1.93-trixie`. A local `clippy -D warnings` on a newer toolchain can
# disagree with CI in both directions, and a gate you cannot reproduce is not
# a gate.
if ! [ -x "$HOME/.cargo/bin/rustup" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --default-toolchain stable \
              --component rustfmt --component clippy
fi
export PATH="$HOME/.cargo/bin:$PATH"
rustup toolchain install 1.93 --component rustfmt --component clippy --profile minimal
rustup default 1.93

# Pinned to the version the `deny` job installs. They are two spellings of one
# number: an unpinned install lets a cargo-deny release change the verdict with
# no commit in this repo to explain it.
command -v cargo-deny >/dev/null || cargo install cargo-deny --version 0.20.2 --locked

say "autologin for $WHO"
# The box must reach a *seated* graphical session with nobody at the console.
# The seat is what supplies XDG_RUNTIME_DIR and a session bus -- and
# XDG_RUNTIME_DIR in particular is where `wm` binds its Wayland socket, which a
# container does not have and which is why `ci/live-tests.sh` has to invent one.
sudo install -d /etc/gdm3
if ! sudo grep -q '^AutomaticLoginEnable=true' /etc/gdm3/daemon.conf 2>/dev/null; then
    sudo sed -i "s/^\[daemon\]/[daemon]\nAutomaticLoginEnable=true\nAutomaticLogin=$WHO/" \
        /etc/gdm3/daemon.conf
fi
sudo systemctl set-default graphical.target

say "done"
cat <<'NEXT'

Reboot, then log in over SSH and run the live tests against the seated session:

    export XDG_RUNTIME_DIR=/run/user/$(id -u)
    export DBUS_SESSION_BUS_ADDRESS=unix:path=$XDG_RUNTIME_DIR/bus
    cargo test -p wm-atspi --test live -- --ignored --test-threads=1

Without those two variables an SSH session finds an empty desktop and reports
no error worth reading. `wm-probe apps` says what the bus can actually see.

To reproduce the CI shape instead, with no desktop involved:

    xvfb-run -a dbus-run-session -- ci/live-tests.sh

GDM defaults to a Wayland session. The README's coordinate findings compare
that against Xorg, which is selectable from the gear menu on the login screen
once gnome-session-xsession is installed -- and needs autologin off, or a
logout, to reach.
NEXT
