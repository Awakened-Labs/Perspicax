/*
 * secctx-launch -- run a command behind a wp_security_context_v1 socket.
 *
 * This is measurement M5, and it is the only candidate for a second attested
 * source that is not a restatement of the first.
 *
 * Sources A (SO_PEERCRED) and B (SCM_CREDENTIALS) both answer "which process",
 * and both are rooted in the same place: whoever holds the socket. Everything
 * derived afterwards -- exe, cgroup, cmdline -- is a function of that pid, so
 * it corroborates rather than adds. The security-context protocol is rooted
 * somewhere else entirely: a launcher creates a fresh listening socket, tells
 * the compositor "everything arriving here is (engine, app_id, instance_id)",
 * and drops the close-fd. The compositor's attestation is then socket
 * provenance -- which listening socket this connection came in on -- and the
 * client cannot reach the manager to contradict it, because the protocol is
 * unavailable on the sandboxed socket by design.
 *
 * That root pre-dates Wine. It is stamped by the launcher before wineserver
 * exists, so it cannot reduce to wineserver's bookkeeping no matter what
 * wineserver later believes about pids and HWNDs.
 *
 * What the spike has to find out is the granularity. Wine fans out into
 * explorer.exe, winemenubuilder.exe, services.exe and the application, and they
 * all inherit one WAYLAND_DISPLAY -- so they all arrive on this one socket and
 * all carry one instance_id. That is the right answer for "which app instance"
 * and useless for "which process", which is exactly why it has to be measured
 * against A and B rather than instead of them.
 *
 *   secctx-launch --app-id com.example.Wine --instance run-1 -- wine app.exe
 */

#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <wayland-client.h>

#include "security-context-v1-client-protocol.h"

static struct wp_security_context_manager_v1 *mgr = NULL;

static void reg_global(void *d, struct wl_registry *r, uint32_t name,
                       const char *iface, uint32_t ver) {
    (void)d; (void)ver;
    if (!strcmp(iface, wp_security_context_manager_v1_interface.name))
        mgr = wl_registry_bind(r, name, &wp_security_context_manager_v1_interface, 1);
}
static void reg_remove(void *d, struct wl_registry *r, uint32_t name) { (void)d; (void)r; (void)name; }
static const struct wl_registry_listener reg_listener = { reg_global, reg_remove };

int main(int argc, char **argv) {
    const char *app_id = "com.example.AttestSpike";
    const char *instance = "instance-1";
    const char *engine = "wl-attest-spike";
    const char *sock_name = "wayland-sandboxed";
    int cmd_at = 0;

    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--app-id") && i + 1 < argc) app_id = argv[++i];
        else if (!strcmp(argv[i], "--instance") && i + 1 < argc) instance = argv[++i];
        else if (!strcmp(argv[i], "--engine") && i + 1 < argc) engine = argv[++i];
        else if (!strcmp(argv[i], "--socket") && i + 1 < argc) sock_name = argv[++i];
        else if (!strcmp(argv[i], "--")) { cmd_at = i + 1; break; }
        else {
            fprintf(stderr, "usage: %s [--app-id ID] [--instance I] [--engine E] "
                            "[--socket NAME] -- command...\n", argv[0]);
            return 2;
        }
    }
    if (!cmd_at || cmd_at >= argc) { fprintf(stderr, "secctx-launch: no command after --\n"); return 2; }

    const char *rt = getenv("XDG_RUNTIME_DIR");
    if (!rt) { fprintf(stderr, "secctx-launch: XDG_RUNTIME_DIR unset\n"); return 2; }

    /* Connect as ourselves -- the launcher, which is trusted precisely because
     * it got here before the sandboxed process did. */
    struct wl_display *dpy = wl_display_connect(NULL);
    if (!dpy) { fprintf(stderr, "secctx-launch: cannot connect to compositor\n"); return 1; }
    struct wl_registry *reg = wl_display_get_registry(dpy);
    wl_registry_add_listener(reg, &reg_listener, NULL);
    wl_display_roundtrip(dpy);

    if (!mgr) {
        fprintf(stderr, "secctx-launch: compositor does not offer "
                        "wp_security_context_manager_v1 -- M5 cannot run here\n");
        return 3;
    }

    /* The socket the sandboxed process will get. The compositor listens on it
     * on our behalf; we never accept on it ourselves. */
    struct sockaddr_un a = { .sun_family = AF_UNIX };
    snprintf(a.sun_path, sizeof a.sun_path, "%s/%s", rt, sock_name);
    unlink(a.sun_path);
    int listen_fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listen_fd < 0 || bind(listen_fd, (struct sockaddr *)&a, sizeof a) < 0
        || listen(listen_fd, 64) < 0) {
        perror("secctx-launch: listen socket");
        return 1;
    }

    /* The close-fd is the sandbox's lifetime. When every copy is gone the
     * compositor tears the context down, so the identity cannot outlive the
     * thing it names and be inherited by something else later. */
    int close_fds[2];
    if (pipe(close_fds) < 0) { perror("pipe"); return 1; }

    struct wp_security_context_v1 *ctx =
        wp_security_context_manager_v1_create_listener(mgr, listen_fd, close_fds[0]);
    wp_security_context_v1_set_sandbox_engine(ctx, engine);
    wp_security_context_v1_set_app_id(ctx, app_id);
    wp_security_context_v1_set_instance_id(ctx, instance);
    wp_security_context_v1_commit(ctx);
    wl_display_roundtrip(dpy);

    close(listen_fd);
    close(close_fds[0]);

    fprintf(stderr, "secctx-launch: socket=%s engine=%s app_id=%s instance_id=%s\n",
            a.sun_path, engine, app_id, instance);

    /* Hand the child the sandboxed socket and nothing else. Note it inherits
     * the write end of the close pipe, which is what keeps the context alive
     * for exactly as long as the child tree does. */
    setenv("WAYLAND_DISPLAY", sock_name, 1);
    unsetenv("WAYLAND_SOCKET");
    execvp(argv[cmd_at], &argv[cmd_at]);
    perror("secctx-launch: exec");
    return 127;
}
