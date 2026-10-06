#!/bin/sh
#
# Install Perspicax as a session a display manager offers: both programs, the
# session entry, and the settings portal's two files.
#
# It builds nothing. Build first, as yourself rather than as root (see "Build"
# in the README):
#
#   cargo build --release --features perspicax/desktop,perspicax-shell/full
#   sudo scripts/install.sh
#
# Where things go, each a variable to override:
#
#   PREFIX      /usr/local
#   DESTDIR     nothing; put before every path, for a package
#   BINDIR      $PREFIX/bin
#   DATADIR     $PREFIX/share
#   SESSIONDIR  $DATADIR/wayland-sessions
#   PORTALDIR   $DATADIR/xdg-desktop-portal/portals
#
# The portal's conf goes in $DATADIR/xdg-desktop-portal, where
# xdg-desktop-portal finds it under any XDG data folder.

set -eu

PREFIX=${PREFIX:-/usr/local}
DESTDIR=${DESTDIR:-}
BINDIR=${BINDIR:-$PREFIX/bin}
DATADIR=${DATADIR:-$PREFIX/share}
SESSIONDIR=${SESSIONDIR:-$DATADIR/wayland-sessions}
PORTALDIR=${PORTALDIR:-$DATADIR/xdg-desktop-portal/portals}

repo=$(cd "$(dirname "$0")/.." && pwd)
built=$repo/target/release

for program in perspicax perspicax-shell; do
    if [ ! -x "$built/$program" ]; then
        echo "install.sh: there is no $built/$program. Build it first, as yourself:" >&2
        echo "  cargo build --release --features perspicax/desktop,perspicax-shell/full" >&2
        exit 1
    fi
done

install -d "$DESTDIR$BINDIR" "$DESTDIR$SESSIONDIR" "$DESTDIR$PORTALDIR" \
    "$DESTDIR$DATADIR/xdg-desktop-portal"
# Side by side: perspicax starts the shell from beside itself.
install -m 755 "$built/perspicax" "$built/perspicax-shell" "$DESTDIR$BINDIR/"
# Full paths, so the greeter finds the program whatever PATH it was given.
sed -e "s|^Exec=perspicax |Exec=$BINDIR/perspicax |" \
    -e "s|^TryExec=perspicax\$|TryExec=$BINDIR/perspicax|" \
    "$repo/dist/perspicax.desktop" >"$DESTDIR$SESSIONDIR/perspicax.desktop"
chmod 644 "$DESTDIR$SESSIONDIR/perspicax.desktop"
install -m 644 "$repo/dist/perspicax.portal" "$DESTDIR$PORTALDIR/"
install -m 644 "$repo/dist/perspicax-portals.conf" "$DESTDIR$DATADIR/xdg-desktop-portal/"

echo "Installed perspicax and perspicax-shell in $BINDIR, and the session entry in $SESSIONDIR."

# Two folders that are read from one place only. Said rather than refused:
# a package, or a machine whose greeter reads /usr/local, may want exactly this.
if [ "$SESSIONDIR" != /usr/share/wayland-sessions ]; then
    cat >&2 <<WARN
install.sh: LightDM, greetd's greeters and older SDDM read session entries only
  from /usr/share/wayland-sessions. If Perspicax is not offered at the greeter,
  link $SESSIONDIR/perspicax.desktop there,
  or install with PREFIX=/usr.
WARN
fi
if [ "$PORTALDIR" != /usr/share/xdg-desktop-portal/portals ]; then
    cat >&2 <<WARN
install.sh: xdg-desktop-portal reads portal files only from
  /usr/share/xdg-desktop-portal/portals, so applications will not follow the
  theme's dark or light until perspicax.portal is there: install with
  PORTALDIR=/usr/share/xdg-desktop-portal/portals, or PREFIX=/usr.
WARN
fi
