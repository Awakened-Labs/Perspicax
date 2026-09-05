/*
 * wl-attest -- a Wayland proxy that records what the kernel says about who is
 * speaking, and forwards everything else untouched.
 *
 * The question this exists to answer is whether a Wine window's identity can
 * come from two kernel-attested sources that do not both reduce to wineserver's
 * bookkeeping. A Unix socket carries two such sources, and they are not the
 * same source stated twice:
 *
 *   A. SO_PEERCRED    -- who called connect(). A snapshot taken once, at
 *                        accept() time, and never refreshed.
 *   B. SCM_CREDENTIALS -- who called sendmsg(), attached by the kernel to every
 *                        message once SO_PASSCRED is set on the receiver.
 *
 * They diverge exactly when the connection outlives the process that opened it:
 * fork, or an fd passed over another socket. A forges nothing and B forges
 * nothing, and neither asks any userspace bookkeeper -- which is the property
 * under test.
 *
 * This is a proxy rather than a compositor on purpose. SO_PEERCRED attestation
 * is compositor-independent, so the measurement must not require our own
 * compositor to exist, and "does winewayland.drv talk to us" is a different
 * question that would eat the budget. Point WAYLAND_DISPLAY here, point here at
 * a stock sway, and the clients cannot tell.
 *
 * What it deliberately does not do: parse the Wayland wire protocol. Object
 * ids, app_ids and titles come from WAYLAND_DEBUG=1 on the client, whose output
 * is already per-process; this ledger carries a byte offset so the two can be
 * joined after the fact. Parsing only becomes necessary if A and B are ever
 * observed to disagree, which is measurement M2.
 *
 * The one thing a byte proxy must get right: Wayland passes file descriptors as
 * SCM_RIGHTS ancillary data -- shm pools, dmabuf, xkb keymaps, sync fds. A
 * splice()-based proxy drops them and the client dies at the first buffer. So
 * every message is recvmsg'd and sendmsg'd with its fds carried across.
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

/* Wayland's own MAX_FDS_OUT. A message never carries more than this. */
#define MAX_FDS 28
#define BUF 4096

static int ledger_fd = -1;

/* One line, one write(), O_APPEND: children racing on the ledger cannot
 * interleave a record, which matters because there is a process per client. */
static void rec(const char *fmt, ...) {
    char line[8192];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(line, sizeof line - 2, fmt, ap);
    va_end(ap);
    if (n < 0) return;
    if (n > (int)sizeof line - 2) n = sizeof line - 2;
    line[n++] = '\n';
    if (ledger_fd >= 0) { ssize_t w = write(ledger_fd, line, n); (void)w; }
    ssize_t w = write(STDERR_FILENO, line, n); (void)w;
}

static double now_s(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

/* JSON string escaping, enough for paths and cmdlines. */
static const char *esc(const char *s, char *out, size_t cap) {
    size_t o = 0;
    if (!s) return "";
    for (; *s && o + 7 < cap; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') { out[o++] = '\\'; out[o++] = (char)c; }
        else if (c < 0x20)         { o += (size_t)snprintf(out + o, cap - o, "\\u%04x", c); }
        else                        { out[o++] = (char)c; }
    }
    out[o] = 0;
    return out;
}

static char *slurp(const char *path, size_t *len_out) {
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return NULL;
    static char b[8192];
    ssize_t n = read(fd, b, sizeof b - 1);
    close(fd);
    if (n < 0) return NULL;
    b[n] = 0;
    if (len_out) *len_out = (size_t)n;
    return b;
}

/* /proc/<pid>/cmdline is NUL-separated; join for logging.
 *
 * Note for whoever reads the ledger: this field is NOT attested. A process can
 * rewrite its own argv, and for a Wine process it is also the only place the
 * Windows executable name appears at all -- /proc/<pid>/exe is the Wine loader
 * for every Wine process alive. That asymmetry is measurement M4, and it is the
 * reason this field is recorded next to the attested ones rather than among
 * them. */
static const char *cmdline_of(pid_t pid, char *out, size_t cap) {
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/cmdline", (int)pid);
    size_t n = 0;
    char *b = slurp(path, &n);
    if (!b) { out[0] = 0; return out; }
    for (size_t i = 0; i + 1 < n; i++) if (b[i] == 0) b[i] = ' ';
    char tmp[8192];
    snprintf(tmp, sizeof tmp, "%s", b);
    esc(tmp, out, cap);
    return out;
}

/* Field 22 of /proc/<pid>/stat. Together with the pid it is a token that
 * survives pid reuse: same pid, different starttime, different process. The
 * pidfd below is the stronger form of the same guard; this is the cheap one
 * that can be written into a ledger and compared later. */
static unsigned long long starttime_of(pid_t pid) {
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/stat", (int)pid);
    char *b = slurp(path, NULL);
    if (!b) return 0;
    /* comm may contain spaces and parens; skip to the last ')' first. */
    char *p = strrchr(b, ')');
    if (!p) return 0;
    p++;
    int field = 2;
    while (*p) {
        while (*p == ' ') p++;
        if (field == 22) return strtoull(p, NULL, 10);
        while (*p && *p != ' ') p++;
        field++;
    }
    return 0;
}

static const char *exe_of(pid_t pid, char *out, size_t cap) {
    char path[64], link[4096];
    snprintf(path, sizeof path, "/proc/%d/exe", (int)pid);
    ssize_t n = readlink(path, link, sizeof link - 1);
    if (n < 0) { out[0] = 0; return out; }
    link[n] = 0;
    return esc(link, out, cap);
}

static const char *cgroup_of(pid_t pid, char *out, size_t cap) {
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/cgroup", (int)pid);
    char *b = slurp(path, NULL);
    if (!b) { out[0] = 0; return out; }
    char *nl = strchr(b, '\n');
    if (nl) *nl = 0;
    char *third = strchr(b, ':');
    if (third) third = strchr(third + 1, ':');
    return esc(third ? third + 1 : b, out, cap);
}

static int pidfd_open_(pid_t pid) {
    return (int)syscall(SYS_pidfd_open, pid, 0);
}

static int connect_upstream(const char *runtime, const char *display) {
    struct sockaddr_un a = { .sun_family = AF_UNIX };
    if (display[0] == '/')
        snprintf(a.sun_path, sizeof a.sun_path, "%s", display);
    else
        snprintf(a.sun_path, sizeof a.sun_path, "%s/%s", runtime, display);
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) return -1;
    if (connect(fd, (struct sockaddr *)&a, sizeof a) < 0) { close(fd); return -1; }
    return fd;
}

struct dir_state {
    unsigned long long bytes;
    pid_t last_pid;      /* last SCM_CREDENTIALS sender seen, -1 if none yet */
    unsigned long long msgs;
    unsigned long long fds;
};

/*
 * Move one message. Returns 0 on success, -1 when the direction has closed.
 *
 * `watch` selects the direction whose per-message credentials we care about:
 * client->server. The other direction is sway talking, whose pid is constant
 * and uninteresting, so it is forwarded without inspection.
 */
static int pump(int from, int to, int watch, int conn, struct dir_state *st) {
    char buf[BUF];
    union {
        char raw[CMSG_SPACE(sizeof(struct ucred)) + CMSG_SPACE(MAX_FDS * sizeof(int))];
        struct cmsghdr align;
    } cm;
    struct iovec iov = { .iov_base = buf, .iov_len = sizeof buf };
    struct msghdr m = { .msg_iov = &iov, .msg_iovlen = 1,
                        .msg_control = cm.raw, .msg_controllen = sizeof cm.raw };

    ssize_t n;
    do { n = recvmsg(from, &m, MSG_CMSG_CLOEXEC); } while (n < 0 && errno == EINTR);
    if (n <= 0) return -1;

    int fds[MAX_FDS];
    int nfds = 0;
    pid_t sender = -1;
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); c; c = CMSG_NXTHDR(&m, c)) {
        if (c->cmsg_level != SOL_SOCKET) continue;
        if (c->cmsg_type == SCM_CREDENTIALS) {
            struct ucred uc;
            memcpy(&uc, CMSG_DATA(c), sizeof uc);
            sender = uc.pid;
        } else if (c->cmsg_type == SCM_RIGHTS) {
            int k = (int)((c->cmsg_len - CMSG_LEN(0)) / sizeof(int));
            if (k > MAX_FDS) k = MAX_FDS;
            memcpy(fds, CMSG_DATA(c), (size_t)k * sizeof(int));
            nfds = k;
        }
    }

    if (watch) {
        st->msgs++;
        st->fds += (unsigned long long)nfds;
        /* Log a transition, not every message: the interesting event is the
         * send-time pid CHANGING, because that is source B disagreeing with
         * source A. Steady state is silent so the ledger stays readable. */
        if (sender != st->last_pid) {
            rec("{\"ev\":\"send_cred\",\"conn\":%d,\"t\":%.6f,\"pid\":%d,"
                "\"prev_pid\":%d,\"at_byte\":%llu,\"msg\":%llu}",
                conn, now_s(), (int)sender, (int)st->last_pid, st->bytes, st->msgs);
            st->last_pid = sender;
        }
    }

    /* Forward. Only SCM_RIGHTS crosses; credentials are ours to read, not to
     * relay, and could not be forged onward anyway. Partial writes keep the fds
     * attached to the first chunk, which preserves Wayland's ordering rule that
     * an fd arrives no later than the message referencing it. */
    size_t off = 0;
    int sent_fds = 0;
    while (off < (size_t)n) {
        struct iovec o = { .iov_base = buf + off, .iov_len = (size_t)n - off };
        struct msghdr s = { .msg_iov = &o, .msg_iovlen = 1 };
        union { char raw[CMSG_SPACE(MAX_FDS * sizeof(int))]; struct cmsghdr align; } oc;
        if (nfds && !sent_fds) {
            memset(oc.raw, 0, sizeof oc.raw);
            s.msg_control = oc.raw;
            s.msg_controllen = CMSG_SPACE((size_t)nfds * sizeof(int));
            struct cmsghdr *c = CMSG_FIRSTHDR(&s);
            c->cmsg_level = SOL_SOCKET;
            c->cmsg_type = SCM_RIGHTS;
            c->cmsg_len = CMSG_LEN((size_t)nfds * sizeof(int));
            memcpy(CMSG_DATA(c), fds, (size_t)nfds * sizeof(int));
        }
        ssize_t w;
        do { w = sendmsg(to, &s, MSG_NOSIGNAL); } while (w < 0 && errno == EINTR);
        if (w < 0) { for (int i = 0; i < nfds; i++) close(fds[i]); return -1; }
        if (nfds && !sent_fds) sent_fds = 1;
        off += (size_t)w;
    }
    for (int i = 0; i < nfds; i++) close(fds[i]);

    st->bytes += (unsigned long long)n;
    return 0;
}

static void serve(int c, int conn, const char *runtime, const char *upstream) {
    /* Source A: taken once, here, and never refreshed. */
    struct ucred uc;
    socklen_t ul = sizeof uc;
    if (getsockopt(c, SOL_SOCKET, SO_PEERCRED, &uc, &ul) < 0) { close(c); return; }

    /* Pin the identity against pid reuse before reading anything out of /proc.
     * Without this the exe/cgroup below belong to whoever holds the pid at the
     * moment we look, which need not be the process that connected. */
    int pidfd = pidfd_open_(uc.pid);
    unsigned long long start = starttime_of(uc.pid);

    char exe[4096], cmd[8192], cg[2048];
    exe_of(uc.pid, exe, sizeof exe);
    cmdline_of(uc.pid, cmd, sizeof cmd);
    cgroup_of(uc.pid, cg, sizeof cg);

    rec("{\"ev\":\"connect\",\"conn\":%d,\"t\":%.6f,\"peercred_pid\":%d,\"uid\":%d,"
        "\"gid\":%d,\"starttime\":%llu,\"pidfd\":%s,\"exe\":\"%s\",\"cgroup\":\"%s\","
        "\"cmdline_unattested\":\"%s\"}",
        conn, now_s(), (int)uc.pid, (int)uc.uid, (int)uc.gid, start,
        pidfd >= 0 ? "true" : "false", exe, cg, cmd);

    int up = connect_upstream(runtime, upstream);
    if (up < 0) {
        rec("{\"ev\":\"upstream_fail\",\"conn\":%d,\"err\":\"%s\"}", conn, strerror(errno));
        close(c);
        return;
    }

    struct dir_state c2s = { .last_pid = -1 }, s2c = { .last_pid = -1 };
    struct pollfd p[2] = { { .fd = c, .events = POLLIN }, { .fd = up, .events = POLLIN } };

    for (;;) {
        if (poll(p, 2, -1) < 0) { if (errno == EINTR) continue; break; }
        if (p[0].revents & POLLIN) { if (pump(c, up, 1, conn, &c2s) < 0) break; }
        if (p[1].revents & POLLIN) { if (pump(up, c, 0, conn, &s2c) < 0) break; }
        if ((p[0].revents | p[1].revents) & (POLLHUP | POLLERR)) {
            /* Drain whatever is still readable before giving up, so the last
             * requests a short-lived client made are not lost. */
            if (p[0].revents & POLLIN) continue;
            break;
        }
    }

    rec("{\"ev\":\"close\",\"conn\":%d,\"t\":%.6f,\"peercred_pid\":%d,"
        "\"c2s_bytes\":%llu,\"c2s_msgs\":%llu,\"c2s_fds\":%llu,\"final_send_pid\":%d}",
        conn, now_s(), (int)uc.pid, c2s.bytes, c2s.msgs, c2s.fds, (int)c2s.last_pid);

    if (pidfd >= 0) close(pidfd);
    close(up);
    close(c);
}

static void reap(int sig) { (void)sig; while (waitpid(-1, NULL, WNOHANG) > 0) {} }

int main(int argc, char **argv) {
    const char *listen_name = "wayland-attest";
    const char *upstream = getenv("WAYLAND_DISPLAY");
    const char *ledger = NULL;

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-l") && i + 1 < argc) listen_name = argv[++i];
        else if (!strcmp(argv[i], "-u") && i + 1 < argc) upstream = argv[++i];
        else if (!strcmp(argv[i], "-o") && i + 1 < argc) ledger = argv[++i];
        else {
            fprintf(stderr,
                "usage: %s [-l listen-name] [-u upstream-display] [-o ledger.jsonl]\n"
                "  listens on $XDG_RUNTIME_DIR/<listen-name>, forwards to <upstream-display>\n",
                argv[0]);
            return 2;
        }
    }
    if (!upstream) { fprintf(stderr, "wl-attest: no upstream (-u or WAYLAND_DISPLAY)\n"); return 2; }

    const char *runtime = getenv("XDG_RUNTIME_DIR");
    if (!runtime) { fprintf(stderr, "wl-attest: XDG_RUNTIME_DIR unset\n"); return 2; }

    if (ledger) {
        ledger_fd = open(ledger, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
        if (ledger_fd < 0) { perror("wl-attest: ledger"); return 1; }
    }

    struct sockaddr_un a = { .sun_family = AF_UNIX };
    snprintf(a.sun_path, sizeof a.sun_path, "%s/%s", runtime, listen_name);
    unlink(a.sun_path);

    int l = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (l < 0) { perror("socket"); return 1; }
    if (bind(l, (struct sockaddr *)&a, sizeof a) < 0) { perror("bind"); return 1; }
    /* Before listen(), so source B is armed before any client can connect. The
     * kernel stamps a message only if the receiving socket had SO_PASSCRED when
     * the message was queued -- set it late and the first requests arrive with
     * pid 0, which is not "sent by pid 0" but "we were not listening yet". That
     * is exactly what the first smoke run recorded, and it would have silently
     * left the earliest surface-creating requests unattested. */
    if (setsockopt(l, SOL_SOCKET, SO_PASSCRED, &(int){1}, sizeof(int)) < 0)
        perror("wl-attest: SO_PASSCRED on listener");
    if (listen(l, 64) < 0) { perror("listen"); return 1; }

    signal(SIGCHLD, reap);
    signal(SIGPIPE, SIG_IGN);

    rec("{\"ev\":\"listen\",\"t\":%.6f,\"socket\":\"%s\",\"upstream\":\"%s\",\"pid\":%d}",
        now_s(), a.sun_path, upstream, (int)getpid());

    /* A process per connection. The alternative is a multiplexer, and the only
     * thing it would buy is a shared address space this program has no use
     * for -- while costing the property that one wedged client cannot stall
     * the ledger for the others. */
    for (int conn = 1;; conn++) {
        int c = accept4(l, NULL, NULL, SOCK_CLOEXEC);
        if (c < 0) { if (errno == EINTR) { conn--; continue; } perror("accept"); break; }
        /* Belt and braces: SO_PASSCRED is not documented as inherited across
         * accept(), so re-arm here, in the parent, before the fork and before
         * the upstream connect() -- the tightest window available. */
        setsockopt(c, SOL_SOCKET, SO_PASSCRED, &(int){1}, sizeof(int));
        pid_t k = fork();
        if (k == 0) { close(l); serve(c, conn, runtime, upstream); _exit(0); }
        close(c);
        if (k < 0) perror("fork");
    }
    return 1;
}
