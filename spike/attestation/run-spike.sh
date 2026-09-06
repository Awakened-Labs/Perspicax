#!/usr/bin/env bash
# Driver for the Wine attestation spike. Subcommands run in the order below.
#
# The whole point of the arrangement is that it never touches perspicax: the
# compositor is a stock sway, the instrument is a proxy on the socket, and the
# clients are ordinary PE binaries. SO_PEERCRED attestation is
# compositor-independent, so testing it against our own compositor would only
# add a variable.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
: "${XDG_RUNTIME_DIR:=/run/user/$(id -u)}"
export XDG_RUNTIME_DIR
export WINEPREFIX="${WINEPREFIX:-$HOME/.wine-attest}"
export WINEDLLOVERRIDES="mscoree,mshtml="   # no gecko/mono prompts
export WINEDEBUG="${WINEDEBUG:--all}"
UPSTREAM="${UPSTREAM:-wayland-1}"
PROXY_SOCK="${PROXY_SOCK:-wayland-attest}"
LEDGER="$HERE/ledger.jsonl"

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

preflight() {
  say "preflight"
  local ok=0
  for b in sway wine x86_64-w64-mingw32-gcc wayland-scanner bwrap; do
    if command -v "$b" >/dev/null; then echo "  ok   $b"; else echo "  MISS $b"; ok=1; fi
  done
  [ -e /usr/share/wayland-protocols/staging/security-context/security-context-v1.xml ] \
    && echo "  ok   security-context-v1.xml" || { echo "  MISS security-context protocol"; ok=1; }
  # A single grep, because a sway without this cannot run M5 at all and the
  # failure otherwise shows up as "nothing was attributed", which reads as a
  # finding rather than a missing feature.
  #
  # Process substitution, not a pipe: `strings ... | grep -q` looks right and is
  # wrong under `set -o pipefail`. grep -q exits at the first match, strings
  # takes SIGPIPE, and the pipeline reports 141 -- so a sway that HAS the
  # feature fails the check that confirms it.
  if grep -q wlr_security_context_manager_v1 <(strings "$(command -v sway)"); then
    echo "  ok   sway has security-context support"
  else
    echo "  MISS sway security-context support -- M5 unavailable"; ok=1
  fi
  [ -z "${LSM:-}" ] && [ ! -s /sys/kernel/security/lsm ] \
    && echo "  note no LSM active -- SO_PEERSEC is not a candidate source here"
  return $ok
}

prefix() {
  say "wineprefix (pay this before the clock starts)"
  wine wineboot -u
  wine reg add 'HKCU\Software\Wine\Drivers' /v Graphics /d wayland /f
  wineserver -w
  echo "  prefix ready at $WINEPREFIX, Graphics=wayland"
}

up() {
  say "compositor + proxy"
  if ! pgrep -x sway >/dev/null; then
    WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
      setsid sway -c "$HERE/sway-headless.conf" >"$HERE/sway.log" 2>&1 &
    sleep 3
  fi
  echo "  sway on $UPSTREAM (headless -- never look at it)"
  pkill -x wl-attest 2>/dev/null || true
  sleep 0.3
  : >"$LEDGER"
  setsid "$HERE/wl-attest" -l "$PROXY_SOCK" -u "$UPSTREAM" -o "$LEDGER" \
    >"$HERE/proxy.log" 2>&1 &
  sleep 1
  echo "  proxy on \$XDG_RUNTIME_DIR/$PROXY_SOCK -> $UPSTREAM, ledger=$LEDGER"
}

down() {
  pkill -x wl-attest 2>/dev/null || true
  pkill -x sway 2>/dev/null || true
  echo "  down"
}

# M2's negative control. Run it FIRST: a zero-divergence result from the Wine
# runs means nothing unless the instrument is known to be able to see one.
control() {
  say "M2 control -- manufacture a divergence and check it is seen"
  WAYLAND_DISPLAY="$PROXY_SOCK" "$HERE/fdpass-probe"
  sleep 0.5
  grep '"ev":"send_cred"' "$LEDGER" | tail -3
}

measure() {
  say "M1/M2/M3/M4 -- census, divergence, granularity, executable"
  # Start clean: the control deliberately puts a non-Wine client and a
  # deliberate divergence in the ledger, and leaving them in makes M4 report two
  # executables and M2 report a divergence that was manufactured rather than
  # observed. The proxy holds this file O_APPEND, so truncating under it is safe.
  : >"$LEDGER"
  export WAYLAND_DISPLAY="$PROXY_SOCK"
  unset DISPLAY
  WAYLAND_DEBUG=1 wine "$HERE/attest-win.exe" --windows 3 --tag multi --seconds 6 \
    >"$HERE/wldebug.log" 2>&1 || true
  wine "$HERE/attest-win.exe" --windows 1 --spawn 2 --tag fan --seconds 6 >/dev/null 2>&1 || true
  wineserver -w
  python3 "$HERE/census.py" "$LEDGER" "$HERE/wldebug.log"
}

m5() {
  say "M5 -- launcher provenance via wp_security_context_v1"
  export SWAYSOCK="$(ls -t "$XDG_RUNTIME_DIR"/sway-ipc.* | head -1)"
  unset DISPLAY
  WAYLAND_DISPLAY="$UPSTREAM" setsid "$HERE/secctx-launch" \
    --app-id com.example.Wine --instance "wine-run-$$" \
    -- wine "$HERE/attest-win.exe" --windows 2 --spawn 2 --tag sbx --seconds 18 \
    >"$HERE/m5.log" 2>&1 &
  sleep 11
  python3 "$HERE/m5-check.py" 'sandbox_app_id="com.example.Wine"'
  wineserver -w
}

all() { preflight; prefix; up; control; measure; m5; }

case "${1:-all}" in
  preflight|prefix|up|down|control|measure|m5|all) "$1" ;;
  *) echo "usage: $0 {preflight|prefix|up|down|control|measure|m5|all}"; exit 2 ;;
esac
