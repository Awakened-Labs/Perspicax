#!/bin/sh
#
# Build Perspicax: `cargo build`, with the features PERSPICAX_BUILD_FEATURES
# names when the command line names none. Every argument is cargo build's:
#
#   scripts/build.sh --release --features perspicax/desktop,perspicax-shell/full
#
#   export PERSPICAX_BUILD_FEATURES=perspicax/desktop,perspicax-shell/full
#   scripts/build.sh --release
#
# Cargo cannot take features from the environment -- it has no setting for
# them, and an alias cannot stand in for `build` -- so a variable exported once
# has to be read by something in front of it. This is that, and nothing more:
# with the variable unset, it is the build `cargo build` is. Forgetting the
# flag does not fail a build, it makes a compositor that runs headless only and
# a shell that draws nothing, which is the mistake the variable is for.

set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)

# Runs of whitespace and commas become one comma, so "a b" and "a, b" reach
# cargo as "a,b" rather than as one feature that does not exist, and a value
# that is only whitespace is unset.
features=$(printf '%s' "${PERSPICAX_BUILD_FEATURES:-}" | tr -s '[:space:],' ',')
features=${features#,}
features=${features%,}

usage() {
    cat <<'EOF'
Build Perspicax: cargo build, with the features PERSPICAX_BUILD_FEATURES names
when the command line names none.

Usage:
  scripts/build.sh [cargo build arguments]

Every argument goes to cargo build as it is; `cargo build --help` lists them.

Env:
  PERSPICAX_BUILD_FEATURES  Cargo features to build with, in the spelling
                            --features takes: comma-separated, and a crate's
                            feature as crate/feature. Export it once and every
                            build carries it. --features, -F or --all-features
                            on the command line wins, so --features '' builds
                            one without it.

Examples:
  scripts/build.sh --release --features perspicax/desktop,perspicax-shell/full

  export PERSPICAX_BUILD_FEATURES=perspicax/desktop,perspicax-shell/full
  scripts/build.sh --release                 # builds with them ...
  scripts/build.sh --release --features ''   # ... and this one without
EOF
    exit 0
}

named=
for arg in "$@"; do
    case $arg in
        -h | --help) usage ;;
        --features | --features=* | -F* | --all-features) named=$arg ;;
    esac
done

if [ -n "$features" ]; then
    if [ -n "$named" ]; then
        echo "build.sh: $named is on the command line, so PERSPICAX_BUILD_FEATURES ($features) is not used" >&2
    else
        echo "build.sh: features $features (from PERSPICAX_BUILD_FEATURES)" >&2
        set -- "$@" --features "$features"
    fi
fi

exec cargo build --manifest-path "$repo/Cargo.toml" "$@"
