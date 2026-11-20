/*
 * hxrt - the few runtime symbols a UMDF driver needs when it is built
 * without the Microsoft C runtime (clang-cl + lld-link, see
 * ci/build-guest-drivers.sh): the WDF stub library references the Control
 * Flow Guard check/dispatch hooks, which the MSVC CRT normally provides.
 * The drivers are linked with /guard:no, so the hooks are the documented
 * no-guard behaviour: "check" returns, "dispatch" jumps to the target in RAX.
 * memcpy/memset/__chkstk come from ntdll.
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
