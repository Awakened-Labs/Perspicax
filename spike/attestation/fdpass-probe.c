/*
 * fdpass-probe -- the negative control for measurement M2.
 *
 * M2 asks whether send-time credentials ever disagree with connect-time ones.
 * A run that reports zero disagreements proves nothing unless the instrument
 * can detect one, so this program manufactures the disagreement on purpose:
 * connect, send a request, fork, and let the CHILD send the next request on the
 * inherited fd. SO_PEERCRED still names the parent forever; SCM_CREDENTIALS
 * names the child from that point on.
 *
 * This is also the threat it stands in for. A Wayland fd is an ordinary fd: it
 * can be inherited across fork or handed to an unrelated process over another
 * Unix socket. Any policy that reads the peer's identity once at connect time
 * and caches it is, after that hand-off, attributing one process's windows to
 * another -- and connect-time attestation cannot see it. Per-message
 * credentials can.
 *
 * It speaks just enough Wayland to be a client: wl_display@1.get_registry is
 * three words and needs no library. The server will object to whatever comes
 * after; the ledger record is written before that matters.
 */

#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

/* wl_display@1.get_registry(new_id) -- 3 words: object, size<<16|opcode, arg. */
static int get_registry(int fd, uint32_t new_id) {
    uint32_t m[3] = { 1u, (12u << 16) | 1u, new_id };
    return (int)send(fd, m, sizeof m, MSG_NOSIGNAL);
}

int main(void) {
    const char *rt = getenv("XDG_RUNTIME_DIR");
    const char *disp = getenv("WAYLAND_DISPLAY");
    if (!rt || !disp) { fprintf(stderr, "fdpass-probe: XDG_RUNTIME_DIR/WAYLAND_DISPLAY unset\n"); return 2; }

    struct sockaddr_un a = { .sun_family = AF_UNIX };
    snprintf(a.sun_path, sizeof a.sun_path, "%s/%s", rt, disp);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0 || connect(fd, (struct sockaddr *)&a, sizeof a) < 0) {
        perror("fdpass-probe: connect");
        return 1;
    }

    printf("fdpass-probe: parent pid=%d connected -- SO_PEERCRED will say this, forever\n", getpid());
    fflush(stdout);
    get_registry(fd, 2);
    usleep(200000);

    pid_t k = fork();
    if (k == 0) {
        /* Same fd, different process. Nothing about the connection changed. */
        printf("fdpass-probe: child  pid=%d sending on the INHERITED fd\n", getpid());
        fflush(stdout);
        get_registry(fd, 3);
        usleep(200000);
        _exit(0);
    }
    waitpid(k, NULL, 0);
    usleep(200000);
    close(fd);
    printf("fdpass-probe: done -- ledger should show a send_cred transition %d -> %d\n", getpid(), k);
    return 0;
}
