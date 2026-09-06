# Everything here is C because the thing under test is a kernel interface, and
# the shortest path to a syscall is the one least likely to be the reason a
# measurement came out the way it did.

CFLAGS  ?= -O2 -Wall -Wextra
WLCLIENT := $(shell pkg-config --cflags --libs wayland-client)
PROTO    := /usr/share/wayland-protocols/staging/security-context/security-context-v1.xml

all: wl-attest fdpass-probe secctx-launch attest-win.exe

wl-attest: wl-attest.c
	$(CC) $(CFLAGS) -o $@ $<

fdpass-probe: fdpass-probe.c
	$(CC) $(CFLAGS) -o $@ $<

security-context-v1-client-protocol.h:
	wayland-scanner client-header $(PROTO) $@

security-context-v1-protocol.c:
	wayland-scanner private-code $(PROTO) $@

secctx-launch: secctx-launch.c security-context-v1-protocol.c security-context-v1-client-protocol.h
	$(CC) $(CFLAGS) -I. -o $@ secctx-launch.c security-context-v1-protocol.c $(WLCLIENT)

# The Win32 side is cross-compiled, not built with winegcc: the point is a
# stock PE binary that Wine has no special knowledge of.
attest-win.exe: attest-win.c
	x86_64-w64-mingw32-gcc -O2 -Wall -o $@ $< -lgdi32

clean:
	rm -f wl-attest fdpass-probe secctx-launch attest-win.exe \
	      security-context-v1-protocol.c security-context-v1-client-protocol.h \
	      *.jsonl *.log

.PHONY: all clean
