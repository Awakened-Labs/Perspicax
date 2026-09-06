# The probe is a stock PE built by mingw, exactly like the attestation spike's
# attest-win.exe -- no Wine headers, nothing Wine-specific. What it measures is
# a property of the application under Wine, not of the toolchain.
CC = x86_64-w64-mingw32-gcc
CFLAGS = -O2 -Wall -Wextra

all: msaa-probe.exe hwnd-diag.exe

msaa-probe.exe: msaa-probe.c
	$(CC) $(CFLAGS) -o $@ $< -loleacc -lole32 -loleaut32 -luuid

# The one that found the answer: the top-level window reports a bare client
# object with no children, because WinForms answers WM_GETOBJECT per control.
hwnd-diag.exe: hwnd-diag.c
	$(CC) $(CFLAGS) -o $@ $< -loleacc -lole32 -loleaut32 -luuid

clean:
	rm -f msaa-probe.exe hwnd-diag.exe WinFormsTarget.exe
.PHONY: all clean
