#!/usr/bin/env bash
#
# The pre-flight, end to end. Deliberately its own prefix: the attestation
# spike's ~/.wine-attest sets WINEDLLOVERRIDES="mscoree,mshtml=" to skip the
# mono prompt, which disables the very thing this measures -- reusing it would
# manufacture an "Absent" verdict out of a configuration choice.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export WINEPREFIX="${WINEPREFIX:-$HOME/.wine-winforms}"
export WINEDEBUG="${WINEDEBUG:--all}"
CSC="$WINEPREFIX/drive_c/windows/Microsoft.NET/Framework/v4.0.30319/csc.exe"

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

preflight() {
  say "preflight"
  local ok=0
  for b in wine x86_64-w64-mingw32-gcc; do
    command -v "$b" >/dev/null && echo "  ok   $b" || { echo "  MISS $b"; ok=1; }
  done
  [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ] \
    && echo "  ok   a display is set" \
    || { echo "  MISS no DISPLAY/WAYLAND_DISPLAY -- the app maps no window and has no tree"; ok=1; }
  return $ok
}

prefix() {
  say "prefix (mono ENABLED -- pay this before the clock starts)"
  wine wineboot -u >/dev/null 2>&1
  wineserver -w
  [ -f "$CSC" ] || { echo "  csc.exe absent: wine-mono did not install"; exit 1; }
  echo "  ready at $WINEPREFIX"
}

build() {
  say "build"
  make -C "$HERE" >/dev/null
  # Built by the prefix's own compiler, against the assemblies it will run on.
  ( cd "$HERE" && wine "$CSC" /nologo /target:winexe /out:WinFormsTarget.exe \
      /r:System.Windows.Forms.dll /r:System.Drawing.dll /r:System.dll \
      WinFormsTarget.cs >/dev/null 2>&1 )
  wineserver -w
  echo "  msaa-probe.exe, hwnd-diag.exe, WinFormsTarget.exe"
}

measure() {
  say "measure"
  ( cd "$HERE" && setsid wine ./WinFormsTarget.exe </dev/null >/dev/null 2>&1 & )
  sleep 12
  echo "--- top-level window (reports a bare client object; see README) ---"
  ( cd "$HERE" && wine ./msaa-probe.exe PreflightTarget 2>/dev/null ) || true
  echo "--- per control HWND (the answer) ---"
  ( cd "$HERE" && wine ./hwnd-diag.exe PreflightTarget 2>/dev/null ) || true
  wineserver -k 2>/dev/null || true
}

case "${1:-all}" in
  preflight) preflight ;;
  prefix)    prefix ;;
  build)     build ;;
  measure)   measure ;;
  all)       preflight && prefix && build && measure ;;
  *) echo "usage: $0 [preflight|prefix|build|measure|all]"; exit 1 ;;
esac
