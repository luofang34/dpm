/*
 * Test support for smoke_terminal.py, loaded with DYLD_INSERT_LIBRARIES into one disposable console.
 *
 * It makes the order in which a console is told its input is ready a fact the test controls, without
 * inventing, dropping or changing any readiness: kevent() and kevent64() return a batch whose order
 * the kernel does not specify, so when one batch holds both the terminal input (event-loop token 0)
 * and the resize notification (token 1), the resize is moved ahead of the input, an order the kernel
 * is allowed to produce. select(), the level-triggered wait used on macOS by the poll backend, is
 * only observed. Everything seen is appended to the file named by DPM_KQ_LOG: a marker that the
 * library loaded, and one line per call that reported the terminal input ready (descriptor 0 for a
 * select, token 0 for a kevent), "both=1" when the resize notification was ready in the same call,
 * at most LOG_LINES lines. Every wrapper returns the real call's result with its errno unchanged.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/event.h>
#include <sys/select.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#define INPUT_TOKEN 0
#define RESIZE_TOKEN 1
#define LOG_LINES 4000
#define MAX_BATCH 16

#define INTERPOSE(replacement, replacee)                                          \
    __attribute__((used)) static struct {                                         \
        const void *replacement_;                                                 \
        const void *replacee_;                                                    \
    } interpose_##replacee __attribute__((section("__DATA,__interpose"))) = {     \
        (const void *)(unsigned long)&replacement, (const void *)(unsigned long)&replacee}

static int log_fd = -2;
static int log_lines = 0;

static void note(const char *text)
{
    if (log_fd == -2) {
        const char *path = getenv("DPM_KQ_LOG");
        log_fd = path ? open(path, O_WRONLY | O_APPEND | O_CREAT, 0600) : -1;
    }
    if (log_fd >= 0 && log_lines < LOG_LINES) {
        log_lines++;
        ssize_t ignored = write(log_fd, text, strlen(text));
        (void)ignored;
    }
}

__attribute__((constructor)) static void loaded(void)
{
    char line[64];
    int saved = errno;
    snprintf(line, sizeof line, "loaded pid=%d\n", (int)getpid());
    note(line);
    errno = saved;
}

struct ready {
    uintptr_t ident;
    int16_t filter;
    uint64_t token;
};

static void describe(char *out, size_t size, const struct ready *batch, int count)
{
    size_t used = 0;
    out[0] = '\0';
    for (int i = 0; i < count && used + 48 < size; i++) {
        used += (size_t)snprintf(out + used, size - used, "[ident=%lu filter=%d token=%llu]",
                                 (unsigned long)batch[i].ident, batch[i].filter,
                                 (unsigned long long)batch[i].token);
    }
}

/* Whether the batch holds both sources; if so, the order that puts the resize first. */
static int plan(const struct ready *batch, int count, int *order)
{
    int input = 0, resize = 0, next = 0;
    for (int i = 0; i < count; i++) {
        input |= batch[i].token == INPUT_TOKEN;
        resize |= batch[i].token == RESIZE_TOKEN;
    }
    if (count < 2 || !input || !resize) {
        return 0;
    }
    for (int i = 0; i < count; i++) {
        if (batch[i].token == RESIZE_TOKEN) {
            order[next++] = i;
        }
    }
    for (int i = 0; i < count; i++) {
        if (batch[i].token != RESIZE_TOKEN) {
            order[next++] = i;
        }
    }
    return 1;
}

static void report(const char *name, const struct ready *batch, const int *order, int count)
{
    char before[1024], after[1024], line[2300];
    struct ready moved[MAX_BATCH];
    for (int i = 0; i < count; i++) {
        moved[i] = batch[order[i]];
    }
    describe(before, sizeof before, batch, count);
    describe(after, sizeof after, moved, count);
    snprintf(line, sizeof line, "%s n=%d both=1 applied=1 before=%s after=%s\n", name, count, before,
             after);
    note(line);
}

static int reordering_kevent(int kq, const struct kevent *changes, int nchanges,
                             struct kevent *events, int nevents, const struct timespec *timeout)
{
    int n = kevent(kq, changes, nchanges, events, nevents, timeout);
    int saved = errno;
    struct ready batch[MAX_BATCH];
    int order[MAX_BATCH];
    if (n < 2 || n > MAX_BATCH) {
        return n;
    }
    for (int i = 0; i < n; i++) {
        batch[i] = (struct ready){events[i].ident, events[i].filter,
                                  (uint64_t)(uintptr_t)events[i].udata};
    }
    if (plan(batch, n, order)) {
        struct kevent copy[MAX_BATCH];
        memcpy(copy, events, (size_t)n * sizeof copy[0]);
        for (int i = 0; i < n; i++) {
            events[i] = copy[order[i]];
        }
        report("kevent", batch, order, n);
    }
    errno = saved;
    return n;
}

static int reordering_kevent64(int kq, const struct kevent64_s *changes, int nchanges,
                               struct kevent64_s *events, int nevents, unsigned int flags,
                               const struct timespec *timeout)
{
    int n = kevent64(kq, changes, nchanges, events, nevents, flags, timeout);
    int saved = errno;
    struct ready batch[MAX_BATCH];
    int order[MAX_BATCH];
    if (n < 2 || n > MAX_BATCH) {
        return n;
    }
    for (int i = 0; i < n; i++) {
        batch[i] = (struct ready){events[i].ident, events[i].filter, events[i].udata};
    }
    if (plan(batch, n, order)) {
        struct kevent64_s copy[MAX_BATCH];
        memcpy(copy, events, (size_t)n * sizeof copy[0]);
        for (int i = 0; i < n; i++) {
            events[i] = copy[order[i]];
        }
        report("kevent64", batch, order, n);
    }
    errno = saved;
    return n;
}

/* Level-triggered readiness has no order to change. A call that reports the terminal input ready is
 * noted with every descriptor it reported ready, and "both=1" if one besides the terminal was. */
static int observing_select(int nfds, fd_set *readfds, fd_set *writefds, fd_set *errorfds,
                            struct timeval *timeout)
{
    int n = select(nfds, readfds, writefds, errorfds, timeout);
    int saved = errno;
    if (n >= 1 && readfds != NULL && nfds <= FD_SETSIZE && FD_ISSET(0, readfds)) {
        char line[160];
        int others = 0;
        size_t used = (size_t)snprintf(line, sizeof line, "select n=%d ready=0", n);
        for (int fd = 1; fd < nfds; fd++) {
            if (FD_ISSET(fd, readfds)) {
                others++;
                if (used + 16 < sizeof line) {
                    used += (size_t)snprintf(line + used, sizeof line - used, ",%d", fd);
                }
            }
        }
        snprintf(line + used, sizeof line - used, " both=%d applied=0\n", others > 0);
        note(line);
    }
    errno = saved;
    return n;
}

INTERPOSE(reordering_kevent, kevent);
INTERPOSE(reordering_kevent64, kevent64);
INTERPOSE(observing_select, select);
