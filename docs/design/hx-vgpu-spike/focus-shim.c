/* focus-shim: LD_PRELOAD for benchmarking under a headless compositor.
 * A headless compositor never gives a window input focus, so games throttle
 * themselves (vkQuake sleeps 16 ms per frame without focus). Report focus,
 * clear "minimized". Benchmark aid only. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdint.h>
typedef uint64_t (*gwf_t)(void *);
uint64_t SDL_GetWindowFlags(void *w) {
    static gwf_t real;
    if (!real) real = (gwf_t)dlsym(RTLD_NEXT, "SDL_GetWindowFlags");
    uint64_t f = real ? real(w) : 0;
    f |= 0x200ULL;   /* SDL_WINDOW_INPUT_FOCUS (SDL3) */
    f &= ~0x40ULL;   /* SDL_WINDOW_MINIMIZED (SDL3) */
    return f;
}
