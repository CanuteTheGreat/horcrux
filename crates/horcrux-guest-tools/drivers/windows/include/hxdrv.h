/*
 * Horcrux guest drivers: the control interface between the Horcrux guest
 * agent (horcrux-guest-agent.exe) and the two user-mode drivers
 *   hxidd - Horcrux virtual display (IddCx indirect display driver)
 *   hxpad - Horcrux virtual gamepad (UMDF HID minidriver)
 *
 * The Rust mirror of these structures is
 * crates/horcrux-guest-tools/src/windows/drivers.rs; keep both in sync
 * (sizes are checked there by unit tests).
 *
 * Written from Microsoft's public IddCx / WDF / HID documentation only.
 * SPDX-License-Identifier: GPL-3.0-or-later
 */
#ifndef HXDRV_H
#define HXDRV_H

#include <stdint.h>

#define HXDRV_ABI_VERSION 1u

/* ---- hxidd ------------------------------------------------------------ */

/* Device interface the agent opens: {6c3b1f4e-2a7d-4c59-9e0b-8d41f2a7c613} */
#define HXIDD_INTERFACE_GUID_INIT \
    { 0x6c3b1f4e, 0x2a7d, 0x4c59, { 0x9e, 0x0b, 0x8d, 0x41, 0xf2, 0xa7, 0xc6, 0x13 } }

/* Fixed buffer sizes of the control structures (ABI, not policy: the real
 * limits are the driver's MaxMonitors setting and the agent's config). */
#define HXIDD_ABI_MONITORS 16u
#define HXIDD_ABI_MODES 32u

/* CTL_CODE(FILE_DEVICE_UNKNOWN, fn, METHOD_BUFFERED, access) */
#define HXDRV_CTL(fn, access) ((0x22u << 16) | ((access) << 14) | ((fn) << 2) | 0u)
#define HXIDD_IOCTL_GET_INFO HXDRV_CTL(0x900u, 1u /* FILE_READ_ACCESS */)
#define HXIDD_IOCTL_SET_MONITORS HXDRV_CTL(0x901u, 2u /* FILE_WRITE_ACCESS */)

typedef struct HXIDD_MODE {
    uint32_t width;
    uint32_t height;
    /* refresh rate in millihertz (240 Hz = 240000) */
    uint32_t refresh_mhz;
} HXIDD_MODE;

/* hxidd monitor flags */
#define HXIDD_MON_SHARE_FRAMES 0x1u /* publish frames as a shared D3D11 texture */

typedef struct HXIDD_MONITOR {
    /* stable id chosen by the agent (0..); becomes the connector index */
    uint32_t id;
    uint32_t flags;
    uint32_t mode_count;
    /* modes[0] is the preferred mode */
    HXIDD_MODE modes[HXIDD_ABI_MODES];
} HXIDD_MONITOR;

/* hxidd set flags */
#define HXIDD_SET_PERSIST 0x1u /* restore this set when the adapter starts (boot, login screen) */

typedef struct HXIDD_SET_MONITORS {
    uint32_t abi;   /* HXDRV_ABI_VERSION */
    uint32_t flags;
    uint32_t count;
    uint32_t reserved;
    HXIDD_MONITOR monitors[HXIDD_ABI_MONITORS];
} HXIDD_SET_MONITORS;

typedef struct HXIDD_MONITOR_INFO {
    uint32_t id;
    uint32_t flags;
    uint32_t width;        /* mode committed by the OS (0 = not active) */
    uint32_t height;
    uint32_t refresh_mhz;
    uint32_t os_target_id;
    uint32_t has_swapchain;
    uint32_t share_generation; /* bumps when the shared texture is recreated */
    uint64_t frames;           /* swap-chain frames processed */
    uint32_t render_luid_low;
    int32_t render_luid_high;
    uint32_t last_error;       /* HRESULT of the last swap-chain failure */
    uint32_t pref_width;       /* preferred mode (modes[0] of the request) */
    uint32_t pref_height;
    uint32_t pref_refresh_mhz;
} HXIDD_MONITOR_INFO;

typedef struct HXIDD_INFO {
    uint32_t abi;
    uint32_t driver_version;   /* major << 16 | minor */
    uint32_t iddcx_version;    /* IddCxGetVersion */
    uint32_t max_monitors;
    uint32_t adapter_ready;
    uint32_t count;
    uint32_t adapter_luid_low;
    int32_t adapter_luid_high;
    HXIDD_MONITOR_INFO monitors[HXIDD_ABI_MONITORS];
} HXIDD_INFO;

#ifdef __cplusplus
static_assert(sizeof(HXIDD_MONITOR_INFO) == 64, "ABI: src/hxdrv.rs IddMonitorInfo");
static_assert(sizeof(HXIDD_INFO) == 32 + HXIDD_ABI_MONITORS * 64, "ABI: src/hxdrv.rs IddInfo");
#endif

/* Shared frame objects (HXIDD_MON_SHARE_FRAMES), {id} = monitor id:
 *   texture  Global\HorcruxIdd{id}Frame{generation}  (B8G8R8A8, keyed mutex, key 0)
 *   event    Global\HorcruxIdd{id}Frame               (auto-reset, set per new frame)
 * Opened by the agent with ID3D11Device1::OpenSharedResourceByName on the
 * render adapter reported in HXIDD_MONITOR_INFO. */
#define HXIDD_SHARED_PREFIX L"Global\\HorcruxIdd"

/* ---- hxpad ------------------------------------------------------------ */

/* The gamepad is a HID device with two top-level collections:
 *   report 1  Generic Desktop / Game Pad   input  (what games read)
 *   report 2  vendor page 0xFF48 usage 1   output (the agent writes state)
 * HID attributes, strings and the vendor page are driver settings. */
#define HXPAD_REPORT_GAMEPAD 1u
#define HXPAD_REPORT_CONTROL 2u

#pragma pack(push, 1)
typedef struct HXPAD_INPUT_REPORT {
    uint8_t report_id;   /* 1 */
    uint16_t buttons;    /* bit 0 = button 1 (A) ... bit 15 = button 16 */
    uint8_t hat;         /* 0 = up, clockwise to 7; 8 = centred (null) */
    int16_t lx, ly;      /* -32767..32767, y down positive (HID convention) */
    int16_t rx, ry;
    uint8_t lt, rt;      /* 0..255 */
} HXPAD_INPUT_REPORT;

typedef struct HXPAD_CONTROL_REPORT {
    uint8_t report_id;   /* 2 */
    uint8_t abi;         /* HXDRV_ABI_VERSION */
    uint16_t buttons;
    uint8_t hat;
    int16_t lx, ly;
    int16_t rx, ry;
    uint8_t lt, rt;
    uint8_t reserved;
} HXPAD_CONTROL_REPORT;
#pragma pack(pop)

#endif
