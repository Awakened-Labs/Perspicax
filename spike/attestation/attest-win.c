/*
 * attest-win -- Win32 top-level windows, on demand, in shapes that separate the
 * questions the spike has to answer.
 *
 * Each measurement needs a different arrangement of processes and windows, and
 * a stock application gives you whichever one it happens to have. So:
 *
 *   --windows N   N top-levels in ONE process. If the kernel can tell these
 *                 apart, window identity is attested; if it cannot, the ceiling
 *                 is the process. (M3)
 *   --spawn M     M child processes, one window each, from one parent. Does
 *                 Wine's fan-out produce a Wayland connection per Win32
 *                 process, or one connection for all of them? (M1)
 *   --tag T       Window titles become "T#0", "T#1", ... so WAYLAND_DEBUG's
 *                 set_title requests can be joined to the ledger's connections
 *                 without this program parsing any protocol.
 *   --seconds S   Exit after S seconds. A spike run should never need a kill.
 *
 * It prints its Win32 pid, which is the OTHER side of the join under test:
 * that number is allocated by wineserver and has no kernel meaning. Whether it
 * can be tied to the SO_PEERCRED pid without asking wineserver is the whole
 * question, so the two are printed and recorded, never assumed equal.
 *
 * No painting. WM_PAINT is left to DefWindowProc and the window may well be
 * blank. A blank window that reached xdg_toplevel is a complete measurement --
 * rendering is explicitly out of scope and is the trap most likely to eat the
 * day.
 */

#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static LRESULT CALLBACK wndproc(HWND h, UINT msg, WPARAM w, LPARAM l) {
    if (msg == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcA(h, msg, w, l);
}

static void spawn_children(int m, const char *tag, int seconds) {
    char self[MAX_PATH];
    GetModuleFileNameA(NULL, self, sizeof self);
    for (int i = 0; i < m; i++) {
        char cmd[MAX_PATH + 128];
        snprintf(cmd, sizeof cmd, "\"%s\" --windows 1 --tag %s-child%d --seconds %d",
                 self, tag, i, seconds);
        STARTUPINFOA si; PROCESS_INFORMATION pi;
        memset(&si, 0, sizeof si); si.cb = sizeof si;
        memset(&pi, 0, sizeof pi);
        if (CreateProcessA(NULL, cmd, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi)) {
            printf("attest-win: spawned child win32_pid=%lu\n", (unsigned long)pi.dwProcessId);
            fflush(stdout);
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        } else {
            printf("attest-win: CreateProcess failed err=%lu\n", (unsigned long)GetLastError());
            fflush(stdout);
        }
    }
}

int main(int argc, char **argv) {
    int windows = 1, spawn = 0, seconds = 20;
    const char *tag = "attest";

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--windows") && i + 1 < argc) windows = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--spawn") && i + 1 < argc) spawn = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--tag") && i + 1 < argc) tag = argv[++i];
        else if (!strcmp(argv[i], "--seconds") && i + 1 < argc) seconds = atoi(argv[++i]);
        else { printf("usage: attest-win [--windows N] [--spawn M] [--tag T] [--seconds S]\n"); return 2; }
    }

    /* Printed before any window exists, so the ledger's connect record for this
     * process can be matched to it even if window creation fails outright. */
    printf("attest-win: tag=%s win32_pid=%lu windows=%d spawn=%d\n",
           tag, (unsigned long)GetCurrentProcessId(), windows, spawn);
    fflush(stdout);

    if (spawn > 0) spawn_children(spawn, tag, seconds);

    WNDCLASSA wc;
    memset(&wc, 0, sizeof wc);
    wc.lpfnWndProc = wndproc;
    wc.hInstance = GetModuleHandleA(NULL);
    wc.lpszClassName = "AttestWin";
    wc.hCursor = LoadCursorA(NULL, IDC_ARROW);
    wc.hbrBackground = (HBRUSH)(COLOR_WINDOW + 1);
    if (!RegisterClassA(&wc)) { printf("attest-win: RegisterClass failed\n"); return 1; }

    for (int i = 0; i < windows; i++) {
        char title[256];
        snprintf(title, sizeof title, "%s#%d", tag, i);
        HWND h = CreateWindowExA(0, "AttestWin", title, WS_OVERLAPPEDWINDOW,
                                 100 + i * 40, 100 + i * 40, 400, 300,
                                 NULL, NULL, wc.hInstance, NULL);
        if (!h) { printf("attest-win: CreateWindow %d failed err=%lu\n", i, (unsigned long)GetLastError()); continue; }
        ShowWindow(h, SW_SHOWNORMAL);
        UpdateWindow(h);
        /* The HWND is wineserver's handle for this window. Printing it next to
         * the title is what lets a reader ask whether anything kernel-side ever
         * distinguishes two rows that differ only here. */
        printf("attest-win: window title=\"%s\" hwnd=%p win32_pid=%lu\n",
               title, (void *)h, (unsigned long)GetCurrentProcessId());
        fflush(stdout);
    }

    SetTimer(NULL, 0, (UINT)(seconds * 1000), NULL);
    MSG msg;
    while (GetMessageA(&msg, NULL, 0, 0)) {
        if (msg.message == WM_TIMER) break;
        TranslateMessage(&msg);
        DispatchMessageA(&msg);
    }
    printf("attest-win: tag=%s exiting\n", tag);
    fflush(stdout);
    return 0;
}
