#!/bin/sh
#
# Install Perspicax as a session a display manager offers: its three programs,
# the session entry, the settings app's entry, and the settings portal's two
# files. `--help` says where each goes, and the variables that move them.
#
# Run it as yourself. It builds first if it has to, through scripts/build.sh,
# and asks sudo only for the folders you cannot write to:
#
#   scripts/install.sh --features perspicax/desktop,perspicax-shell/full
#
#   export PERSPICAX_BUILD_FEATURES=perspicax/desktop,perspicax-shell/full
#   scripts/install.sh
#
# It builds when there is no build to install, or when this run names
# features. Naming them every time is what the variable is for, and costs
# nothing when the build is current: cargo checks and stops. A run that names
# none installs the build target/release holds, so one made by hand with
# `cargo build --release --features ...` is installed as it is, never rebuilt
# without its features. With no build and nothing named, it builds what a
# session needs.
#
# As root -- `sudo scripts/install.sh`, or a package -- it builds nothing. A
# build as root leaves target/ owned by root, and sudo drops an exported
# PERSPICAX_BUILD_FEATURES before this could read it anyway.

set -eu

PREFIX=${PREFIX:-/usr/local}
DESTDIR=${DESTDIR:-}
BINDIR=${BINDIR:-$PREFIX/bin}
DATADIR=${DATADIR:-$PREFIX/share}
SESSIONDIR=${SESSIONDIR:-$DATADIR/wayland-sessions}
PORTALDIR=${PORTALDIR:-$DATADIR/xdg-desktop-portal/portals}

repo=$(cd "$(dirname "$0")/.." && pwd)
built=${CARGO_TARGET_DIR:-$repo/target}/release

# What a session needs: a seat, Xwayland and capture, and a shell with every
# component. Without them there is a compositor that runs headless only and a
# shell that draws nothing.
SESSION_FEATURES=perspicax/desktop,perspicax-shell/full
# The settings app, built in a cargo run of its own: its window puts every
# Wayland client in a build onto libwayland, and the shell is kept off it.
SETTINGS_FEATURES=perspicax-settings/gui
APPDIR=${APPDIR:-$DATADIR/applications}

# The features this run names -- --features, then PERSPICAX_BUILD_FEATURES --
# spelled as build.sh spells them, and `from` for which of the two named them.
features=$(printf '%s' "${PERSPICAX_BUILD_FEATURES:-}" | tr -s '[:space:],' ',')
features=${features#,}
features=${features%,}
from=
[ -z "$features" ] || from=PERSPICAX_BUILD_FEATURES

die() {
    echo "install.sh: $*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Install Perspicax as a session a display manager offers: perspicax,
perspicax-shell and perspicax-settings, the session entry, the settings app's
entry, and the settings portal's two files.

Usage:
  scripts/install.sh [--features <list>]

Run it as yourself: it builds first if it has to, through scripts/build.sh,
and uses sudo only for the folders you cannot write to. It builds when there is
no build in target/release, or when this run names features; a run that names
none installs the build that is there. As root it builds nothing.

Flags:
  --features <list>  Cargo features to build with, comma-separated, in the
                     spelling `cargo build` takes. A session wants
                     perspicax/desktop,perspicax-shell/full, which is what it
                     builds when there is no build and nothing is named.
                     Overrides PERSPICAX_BUILD_FEATURES; --features '' builds
                     without them. perspicax-settings is built apart, with
                     its own feature, whatever this names.
  -h, --help         Show this help and exit.

Env:
  PERSPICAX_BUILD_FEATURES  Cargo features to build with, as --features.
                            Export it once and every install builds with it.
  PREFIX      /usr/local
  DESTDIR     nothing; put before every path, for a package
  BINDIR      $PREFIX/bin
  DATADIR     $PREFIX/share
  SESSIONDIR  $DATADIR/wayland-sessions
  PORTALDIR   $DATADIR/xdg-desktop-portal/portals
  APPDIR      $DATADIR/applications, for the settings app's entry

The portal's conf goes in $DATADIR/xdg-desktop-portal, where
xdg-desktop-portal finds it under any XDG data folder.

Examples:
  scripts/install.sh --features perspicax/desktop,perspicax-shell/full
  PREFIX=/usr scripts/install.sh --features perspicax/desktop,perspicax-shell/full

  export PERSPICAX_BUILD_FEATURES=perspicax/desktop,perspicax-shell/full
  scripts/install.sh           # builds with them, then installs
  sudo scripts/install.sh      # installs what is built, building nothing
EOF
    exit 0
}

while [ $# -gt 0 ]; do
    case $1 in
        --features)
            [ $# -ge 2 ] || die "--features needs a value, e.g. --features $SESSION_FEATURES"
            features=$2
            from=--features
            shift
            ;;
        --features=*)
            features=${1#--features=}
            from=--features
            ;;
        -h | --help) usage ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
    shift
done

missing=
for program in perspicax perspicax-shell perspicax-settings; do
    [ -x "$built/$program" ] || missing=${missing:-$program}
done

if [ "$(id -u)" -eq 0 ]; then
    if [ -n "$missing" ]; then
        cat >&2 <<EOF
install.sh: there is no $built/$missing, and as root this builds
  nothing. Build as yourself first, then install as root:
  scripts/build.sh --release --features $SESSION_FEATURES
  sudo scripts/install.sh
Or run this as yourself, which builds and then uses sudo for the copy.
EOF
        exit 1
    fi
    if [ -n "$from" ]; then
        echo "install.sh: as root this builds nothing, so it installs what $built holds, not a build with '$features' (from $from)" >&2
    fi
elif [ -n "$from" ] || [ -n "$missing" ]; then
    # build.sh reads the variable itself, and says it did.
    case $from in
        --features) "$repo/scripts/build.sh" --release --features "$features" ;;
        PERSPICAX_BUILD_FEATURES) "$repo/scripts/build.sh" --release ;;
        *)
            echo "install.sh: there is no build, and no features named: building a session's, $SESSION_FEATURES" >&2
            "$repo/scripts/build.sh" --release --features "$SESSION_FEATURES"
            ;;
    esac
    "$repo/scripts/build.sh" --release -p perspicax-settings --features "$SETTINGS_FEATURES"
fi

# Whether a folder can be written: it, or the nearest folder above it that
# exists, which `install -d` would make it in.
writable() {
    dir=$1
    while [ ! -e "$dir" ]; do
        dir=$(dirname "$dir")
    done
    [ -w "$dir" ]
}

# sudo for the copy alone, and only when a folder needs it: a PREFIX or
# DESTDIR of your own needs none.
sudo=
if [ "$(id -u)" -ne 0 ]; then
    for folder in "$DESTDIR$BINDIR" "$DESTDIR$SESSIONDIR" "$DESTDIR$PORTALDIR" \
        "$DESTDIR$DATADIR/xdg-desktop-portal" "$DESTDIR$APPDIR"; do
        if [ -z "$sudo" ] && ! writable "$folder"; then
            echo "install.sh: you cannot write to $folder, so the copy uses sudo" >&2
            sudo=sudo
        fi
    done
fi

$sudo install -d "$DESTDIR$BINDIR" "$DESTDIR$SESSIONDIR" "$DESTDIR$PORTALDIR" \
    "$DESTDIR$DATADIR/xdg-desktop-portal" "$DESTDIR$APPDIR"
# Side by side: perspicax starts the shell from beside itself, and the
# settings app finds the perspicax that checks what it writes there.
$sudo install -m 755 "$built/perspicax" "$built/perspicax-shell" \
    "$built/perspicax-settings" "$DESTDIR$BINDIR/"
# Full paths, so the greeter finds the program whatever PATH it was given.
# Written as you and copied by `install`, which is what sudo can run.
entry=$(mktemp)
trap 'rm -f "$entry"' EXIT
sed -e "s|^Exec=perspicax |Exec=$BINDIR/perspicax |" \
    -e "s|^TryExec=perspicax\$|TryExec=$BINDIR/perspicax|" \
    "$repo/dist/perspicax.desktop" >"$entry"
$sudo install -m 644 "$entry" "$DESTDIR$SESSIONDIR/perspicax.desktop"
sed -e "s|^Exec=perspicax-settings\$|Exec=$BINDIR/perspicax-settings|" \
    -e "s|^TryExec=perspicax-settings\$|TryExec=$BINDIR/perspicax-settings|" \
    "$repo/dist/perspicax-settings.desktop" >"$entry"
$sudo install -m 644 "$entry" "$DESTDIR$APPDIR/perspicax-settings.desktop"
$sudo install -m 644 "$repo/dist/perspicax.portal" "$DESTDIR$PORTALDIR/"
$sudo install -m 644 "$repo/dist/perspicax-portals.conf" "$DESTDIR$DATADIR/xdg-desktop-portal/"

echo "Installed perspicax, perspicax-shell and perspicax-settings in $BINDIR, the session entry in $SESSIONDIR, and the settings app's in $APPDIR."

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
