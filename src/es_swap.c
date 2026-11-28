/* The ES-renderer write hook for rust-modules/src/player/es_carrier.rs.
 *
 * LG's libkadaptor runs in this process and writes IEC 61937 bursts into its ES renderer with
 * ioctl(fd, 0x40285207, arg). The executable exports this ioctl (Makefile: --export-dynamic-symbol),
 * so that call binds here; every call is forwarded with the raw syscall.
 *
 * Only while a playback has armed the carrier (nj_es_armed, set by es_carrier::set_on, and only on
 * the firmware the layout was measured on) is the argument read at all. An ES-renderer write
 * (word 8 == 1) is then offered to nj_es_swap, which may point words 0 and 2 at a queued burst;
 * nj_es_swap_done puts liblxa's own words back after the syscall. */
#include <stdarg.h>
#include <sys/syscall.h>
#include <unistd.h>

extern volatile int nj_es_armed;
void nj_es_swap(unsigned *w);
void nj_es_swap_done(unsigned *w, int ret);

int ioctl(int fd, unsigned long req, ...) {
    va_list ap;
    va_start(ap, req);
    void *arg = va_arg(ap, void *);
    va_end(ap);
    unsigned *w = arg;
    int es = nj_es_armed && req == 0x40285207UL && w && w[8] == 1;
    if (es)
        nj_es_swap(w);
    int ret = (int)syscall(SYS_ioctl, fd, req, arg);
    if (es)
        nj_es_swap_done(w, ret);
    return ret;
}
