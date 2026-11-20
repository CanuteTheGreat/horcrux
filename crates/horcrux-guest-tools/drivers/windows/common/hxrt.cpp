/*
 * hxrt - the few runtime symbols a UMDF driver needs when it is built
 * without the Microsoft C runtime (clang-cl + lld-link, see
 * ci/build-guest-drivers.sh): the WDF stub library references the Control
 * Flow Guard check/dispatch hooks, which the MSVC CRT normally provides.
 * The drivers are linked with /guard:no, so the hooks are the documented
 * no-guard behaviour: "check" returns, "dispatch" jumps to the target in RAX.
 * The SDK's ntdll.lib exports none of the C string/memory routines, so the
 * ones a compiler may emit calls to on its own (struct zeroing/copies, or a
 * loop it recognises as wcslen, as clang 21 does) are defined below. They are
 * marked no_builtin so the optimizer cannot turn their loops back into calls
 * to themselves.
 * SPDX-License-Identifier: GPL-3.0-or-later
 */
extern "C" {

__attribute__((naked)) void hx_guard_check_nop(void)
{
    __asm__ volatile("ret");
}

__attribute__((naked)) void hx_guard_dispatch_nop(void)
{
    __asm__ volatile("jmp *%rax");
}

void *__guard_check_icall_fptr = (void *)hx_guard_check_nop;
void *__guard_dispatch_icall_fptr = (void *)hx_guard_dispatch_nop;
}

/* Symbols the stub library names directly (some with characters C++ cannot
 * spell), defined as aliases of the two hooks above. */
__asm__(".globl _guard_check_icall\n"
        ".set _guard_check_icall, hx_guard_check_nop\n"
        ".globl _guard_check_icall_nop\n"
        ".set _guard_check_icall_nop, hx_guard_check_nop\n"
        ".globl \"_guard_check_icall_$fo$\"\n"
        ".set \"_guard_check_icall_$fo$\", hx_guard_check_nop\n"
        ".globl _guard_dispatch_icall\n"
        ".set _guard_dispatch_icall, hx_guard_dispatch_nop\n"
        ".globl _guard_dispatch_icall_nop\n"
        ".set _guard_dispatch_icall_nop, hx_guard_dispatch_nop\n"
        ".globl \"_guard_dispatch_icall_$fo$\"\n"
        ".set \"_guard_dispatch_icall_$fo$\", hx_guard_dispatch_nop\n"
        ".globl _guard_xfg_dispatch_icall_nop\n"
        ".set _guard_xfg_dispatch_icall_nop, hx_guard_dispatch_nop\n");

extern "C" {

__attribute__((no_builtin)) void *memset(void *dst, int c, __SIZE_TYPE__ n)
{
    unsigned char *d = (unsigned char *)dst;
    while (n--) *d++ = (unsigned char)c;
    return dst;
}

__attribute__((no_builtin)) void *memcpy(void *dst, const void *src, __SIZE_TYPE__ n)
{
    unsigned char *d = (unsigned char *)dst;
    const unsigned char *s = (const unsigned char *)src;
    while (n--) *d++ = *s++;
    return dst;
}

__attribute__((no_builtin)) void *memmove(void *dst, const void *src, __SIZE_TYPE__ n)
{
    unsigned char *d = (unsigned char *)dst;
    const unsigned char *s = (const unsigned char *)src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else {
        while (n--) d[n] = s[n];
    }
    return dst;
}

__attribute__((no_builtin)) int memcmp(const void *a, const void *b, __SIZE_TYPE__ n)
{
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    for (; n; n--, x++, y++)
        if (*x != *y) return *x < *y ? -1 : 1;
    return 0;
}

__attribute__((no_builtin)) __SIZE_TYPE__ strlen(const char *s)
{
    __SIZE_TYPE__ n = 0;
    while (s[n]) n++;
    return n;
}

__attribute__((no_builtin)) __SIZE_TYPE__ wcslen(const wchar_t *s)
{
    __SIZE_TYPE__ n = 0;
    while (s[n]) n++;
    return n;
}
}
