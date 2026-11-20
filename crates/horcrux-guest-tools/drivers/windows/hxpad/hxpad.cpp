/*
 * hxpad - Horcrux virtual gamepad: a UMDF 2 HID minidriver.
 *
 * Windows ships no way for a user-mode program to create a game controller,
 * so Horcrux guest tools install this driver. It sits as the lower filter
 * under the inbox HID pass-through driver (MsHidUmdf.sys) and presents a HID
 * device with two top-level collections:
 *   report 1  Generic Desktop / Game Pad (input): 16 buttons, hat, two
 *             16-bit sticks (X/Y, Rx/Ry), two 8-bit triggers (Z/Rz) - what
 *             DirectInput, Raw Input, Windows.Gaming.Input RawGameController,
 *             GameInput and SDL read;
 *   report 2  vendor collection (output): the Horcrux guest agent writes the
 *             state the node received from the viewer's gamepad.
 * XInput only talks to Xbox-class devices; this driver does not pretend to be
 * one (no borrowed vendor/product ids). HID attributes and strings are
 * settings in the device's hardware key (INF defaults).
 *
 * Clean-room: written from Microsoft's WDF/HID documentation and the USB HID
 * specification only (no Windows-driver-samples code).
 * SPDX-License-Identifier: GPL-3.0-or-later
 */
#define WIN32_LEAN_AND_MEAN
#define UMDF_USING_NTSTATUS
#include <windows.h>
#include <ntstatus.h>
#include <devpropdef.h>
#include <wdf.h>
#include <hidclass.h>

#include "hxdrv.h"

/* --- HID minidriver interface (hidport.h), restated from the documentation */
#define HXP_HID_CTL(id) CTL_CODE(FILE_DEVICE_KEYBOARD, (id), METHOD_NEITHER, FILE_ANY_ACCESS)
#define HXP_IOCTL_GET_DEVICE_DESCRIPTOR HXP_HID_CTL(0)
#define HXP_IOCTL_GET_REPORT_DESCRIPTOR HXP_HID_CTL(1)
#define HXP_IOCTL_READ_REPORT HXP_HID_CTL(2)
#define HXP_IOCTL_WRITE_REPORT HXP_HID_CTL(3)
#define HXP_IOCTL_GET_STRING HXP_HID_CTL(4)
#define HXP_IOCTL_ACTIVATE_DEVICE HXP_HID_CTL(7)
#define HXP_IOCTL_DEACTIVATE_DEVICE HXP_HID_CTL(8)
#define HXP_IOCTL_GET_DEVICE_ATTRIBUTES HXP_HID_CTL(9)
#define HXP_IOCTL_UMDF_SET_FEATURE HXP_HID_CTL(20)
#define HXP_IOCTL_UMDF_GET_FEATURE HXP_HID_CTL(21)
#define HXP_IOCTL_UMDF_SET_OUTPUT_REPORT HXP_HID_CTL(22)
#define HXP_IOCTL_UMDF_GET_INPUT_REPORT HXP_HID_CTL(23)
#define HXP_STRING_MANUFACTURER 14
#define HXP_STRING_PRODUCT 15
#define HXP_STRING_SERIAL 16

#pragma pack(push, 1)
struct HxpHidDescriptor {
    UCHAR bLength;
    UCHAR bDescriptorType; /* 0x21 */
    USHORT bcdHID;
    UCHAR bCountry;
    UCHAR bNumDescriptors;
    UCHAR bReportType; /* 0x22 */
    USHORT wReportLength;
};
#pragma pack(pop)

struct HxpAttributes {
    ULONG Size;
    USHORT VendorID;
    USHORT ProductID;
    USHORT VersionNumber;
    USHORT Reserved[11];
};

/* HID report descriptor (USB HID 1.11 items; layout = HXPAD_INPUT_REPORT /
 * HXPAD_CONTROL_REPORT in hxdrv.h). */
static const UCHAR kReportDescriptor[] = {
    0x05, 0x01,       /* Usage Page (Generic Desktop) */
    0x09, 0x05,       /* Usage (Game Pad) */
    0xA1, 0x01,       /* Collection (Application) */
    0x85, HXPAD_REPORT_GAMEPAD, /* Report ID */
    0x05, 0x09,       /*   Usage Page (Button) */
    0x19, 0x01,       /*   Usage Minimum (1) */
    0x29, 0x10,       /*   Usage Maximum (16) */
    0x15, 0x00,       /*   Logical Minimum (0) */
    0x25, 0x01,       /*   Logical Maximum (1) */
    0x75, 0x01,       /*   Report Size (1) */
    0x95, 0x10,       /*   Report Count (16) */
    0x81, 0x02,       /*   Input (Data,Var,Abs) */
    0x05, 0x01,       /*   Usage Page (Generic Desktop) */
    0x09, 0x39,       /*   Usage (Hat switch) */
    0x15, 0x00,       /*   Logical Minimum (0) */
    0x25, 0x07,       /*   Logical Maximum (7) */
    0x35, 0x00,       /*   Physical Minimum (0) */
    0x46, 0x3B, 0x01, /*   Physical Maximum (315) */
    0x65, 0x14,       /*   Unit (degrees) */
    0x75, 0x04,       /*   Report Size (4) */
    0x95, 0x01,       /*   Report Count (1) */
    0x81, 0x42,       /*   Input (Data,Var,Abs,Null state) */
    0x65, 0x00,       /*   Unit (none) */
    0x75, 0x04,       /*   Report Size (4) */
    0x95, 0x01,       /*   Report Count (1) */
    0x81, 0x03,       /*   Input (Const) padding */
    0x09, 0x30,       /*   Usage (X) */
    0x09, 0x31,       /*   Usage (Y) */
    0x09, 0x33,       /*   Usage (Rx) */
    0x09, 0x34,       /*   Usage (Ry) */
    0x16, 0x01, 0x80, /*   Logical Minimum (-32767) */
    0x26, 0xFF, 0x7F, /*   Logical Maximum (32767) */
    0x75, 0x10,       /*   Report Size (16) */
    0x95, 0x04,       /*   Report Count (4) */
    0x81, 0x02,       /*   Input (Data,Var,Abs) */
    0x09, 0x32,       /*   Usage (Z)  left trigger */
    0x09, 0x35,       /*   Usage (Rz) right trigger */
    0x15, 0x00,       /*   Logical Minimum (0) */
    0x26, 0xFF, 0x00, /*   Logical Maximum (255) */
    0x75, 0x08,       /*   Report Size (8) */
    0x95, 0x02,       /*   Report Count (2) */
    0x81, 0x02,       /*   Input (Data,Var,Abs) */
    0xC0,             /* End Collection */

    0x06, 0x48, 0xFF, /* Usage Page (vendor 0xFF48, Horcrux) */
    0x09, 0x01,       /* Usage (1) */
    0xA1, 0x01,       /* Collection (Application) */
    0x85, HXPAD_REPORT_CONTROL, /* Report ID */
    0x09, 0x02,       /*   Usage (2) */
    0x15, 0x00,       /*   Logical Minimum (0) */
    0x26, 0xFF, 0x00, /*   Logical Maximum (255) */
    0x75, 0x08,       /*   Report Size (8) */
    0x95, sizeof(HXPAD_CONTROL_REPORT) - 1, /* Report Count */
    0x91, 0x02,       /*   Output (Data,Var,Abs) */
    0xC0,             /* End Collection */
};

static_assert(sizeof(HXPAD_INPUT_REPORT) == 14, "input report layout");
static_assert(sizeof(HXPAD_CONTROL_REPORT) == 16, "control report layout");

/* Defaults for the settings in the hardware key. 0x0000 vendor id: no
 * vendor id is claimed (see the INF). */
#define HXPAD_DEFAULT_VID 0x0000
#define HXPAD_DEFAULT_PID 0x0000
#define HXPAD_DEFAULT_VERSION 0x0100

struct HxPad {
    WDFDEVICE wdf;
    WDFQUEUE reads; /* pended IOCTL_HID_READ_REPORT */
    CRITICAL_SECTION lock;
    HXPAD_INPUT_REPORT state;
    bool changed;
    USHORT vid, pid, version;
    WCHAR manufacturer[64];
    WCHAR product[64];
    WCHAR serial[200];
};

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(HxPad, HxGetPad);

extern "C" DRIVER_INITIALIZE DriverEntry;
static EVT_WDF_DRIVER_DEVICE_ADD HxpDeviceAdd;
static EVT_WDF_IO_QUEUE_IO_DEVICE_CONTROL HxpIoctl;

#ifdef HX_TRACE
/* debug builds only (-DHX_TRACE): append steps to C:\ProgramData\HorcruxTrace\hxpad.log */
static void HxpTrace(const char *what, NTSTATUS st)
{
    HANDLE h = CreateFileW(L"C:\\ProgramData\\HorcruxTrace\\hxpad.log", FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                           NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (h == INVALID_HANDLE_VALUE) return;
    char line[160];
    int n = 0;
    while (what[n] && n < 120) { line[n] = what[n]; n++; }
    line[n++] = ' ';
    for (int i = 7; i >= 0; i--) line[n++] = "0123456789abcdef"[((ULONG)st >> (i * 4)) & 0xf];
    line[n++] = '\r'; line[n++] = '\n';
    DWORD w;
    WriteFile(h, line, n, &w, NULL);
    CloseHandle(h);
}
#define HXT(w, st) HxpTrace(w, st)
#else
#define HXT(w, st) ((void)0)
#endif

static void HxpZero(void *p, size_t n)
{
    volatile unsigned char *b = (volatile unsigned char *)p;
    while (n--) *b++ = 0;
}

static void HxpCopy(void *d, const void *s, size_t n)
{
    unsigned char *a = (unsigned char *)d;
    const unsigned char *b = (const unsigned char *)s;
    while (n--) *a++ = *b++;
}

static void HxpStr(WCHAR *dst, size_t cap, const WCHAR *src)
{
    size_t i = 0;
    while (src[i] && i + 1 < cap) {
        dst[i] = src[i];
        i++;
    }
    dst[i] = 0;
}

static size_t HxpLen(const WCHAR *s)
{
    size_t n = 0;
    while (s[n]) n++;
    return n;
}

static void HxpReadSettings(HxPad *p)
{
    WDFKEY key;
    p->vid = HXPAD_DEFAULT_VID;
    p->pid = HXPAD_DEFAULT_PID;
    p->version = HXPAD_DEFAULT_VERSION;
    HxpStr(p->manufacturer, 64, L"Horcrux");
    HxpStr(p->product, 64, L"Horcrux Virtual Gamepad");
    if (!NT_SUCCESS(WdfDeviceOpenRegistryKey(p->wdf, PLUGPLAY_REGKEY_DEVICE, KEY_READ,
                                             WDF_NO_OBJECT_ATTRIBUTES, &key)))
        return;
    struct { PCWSTR name; USHORT *v; } nums[] = {
        { L"HidVendorId", &p->vid }, { L"HidProductId", &p->pid }, { L"HidVersion", &p->version } };
    for (auto &n : nums) {
        UNICODE_STRING u;
        ULONG v;
        u.Buffer = (PWSTR)n.name;
        u.Length = u.MaximumLength = (USHORT)(HxpLen(n.name) * sizeof(WCHAR));
        if (NT_SUCCESS(WdfRegistryQueryULong(key, &u, &v))) *n.v = (USHORT)v;
    }
    struct { PCWSTR name; WCHAR *buf; } strs[] = {
        { L"HidManufacturer", p->manufacturer }, { L"HidProduct", p->product } };
    for (auto &s : strs) {
        UNICODE_STRING u, val;
        WCHAR tmp[64];
        u.Buffer = (PWSTR)s.name;
        u.Length = u.MaximumLength = (USHORT)(HxpLen(s.name) * sizeof(WCHAR));
        val.Buffer = tmp;
        val.Length = 0;
        val.MaximumLength = sizeof(tmp) - sizeof(WCHAR);
        if (NT_SUCCESS(WdfRegistryQueryUnicodeString(key, &u, NULL, &val))) {
            tmp[val.Length / sizeof(WCHAR)] = 0;
            HxpStr(s.buf, 64, tmp);
        }
    }
    WdfRegistryClose(key);
}

/* The serial number string is the device instance id (e.g.
 * SWD\HorcruxEnum\HxPad0): the agent finds pad N by it. */
static void HxpReadSerial(HxPad *p)
{
    DEVPROPKEY key = { { 0x78c34fc8, 0x104a, 0x4aca, { 0x9e, 0xa4, 0x52, 0x4d, 0x52, 0x99, 0x6e, 0x57 } }, 256 };
    WDF_DEVICE_PROPERTY_DATA data;
    WDFMEMORY mem = NULL;
    DEVPROPTYPE type;
    HxpStr(p->serial, 200, L"HxPad");
    WDF_DEVICE_PROPERTY_DATA_INIT(&data, &key);
    if (NT_SUCCESS(WdfDeviceAllocAndQueryPropertyEx(p->wdf, &data, NonPagedPoolNx, WDF_NO_OBJECT_ATTRIBUTES,
                                                    &mem, &type)) &&
        type == DEVPROP_TYPE_STRING) {
        size_t len = 0;
        WCHAR *s = (WCHAR *)WdfMemoryGetBuffer(mem, &len);
        if (s && len >= sizeof(WCHAR)) {
            size_t n = len / sizeof(WCHAR);
            WCHAR tmp[200];
            size_t i = 0;
            for (; i < n && i + 1 < 200 && s[i]; i++) tmp[i] = s[i];
            tmp[i] = 0;
            HxpStr(p->serial, 200, tmp);
        }
    }
    if (mem) WdfObjectDelete(mem);
}

static NTSTATUS HxpOut(WDFREQUEST req, const void *src, size_t len, ULONG_PTR *info)
{
    PVOID buf;
    size_t cap;
    NTSTATUS st = WdfRequestRetrieveOutputBuffer(req, len, &buf, &cap);
    if (!NT_SUCCESS(st)) return st;
    HxpCopy(buf, src, len);
    *info = len;
    return STATUS_SUCCESS;
}

/* Complete one pended read with the current state (caller holds lock). */
static void HxpPump(HxPad *p)
{
    WDFREQUEST r;
    if (!p->changed) return;
    if (!NT_SUCCESS(WdfIoQueueRetrieveNextRequest(p->reads, &r))) return;
    ULONG_PTR info = 0;
    NTSTATUS st = HxpOut(r, &p->state, sizeof(p->state), &info);
    p->changed = false;
    WdfRequestCompleteWithInformation(r, st, info);
}

static NTSTATUS HxpControl(HxPad *p, WDFREQUEST req, ULONG_PTR *info)
{
    PVOID buf;
    size_t len;
    NTSTATUS st = WdfRequestRetrieveInputBuffer(req, 1, &buf, &len);
    if (!NT_SUCCESS(st)) return st;
    const UCHAR *b = (const UCHAR *)buf;
    HXPAD_CONTROL_REPORT c;
    /* the report may arrive with or without its report id byte */
    if (len >= sizeof(c) && b[0] == HXPAD_REPORT_CONTROL) {
        HxpCopy(&c, b, sizeof(c));
    } else if (len >= sizeof(c) - 1) {
        c.report_id = HXPAD_REPORT_CONTROL;
        HxpCopy(((UCHAR *)&c) + 1, b, sizeof(c) - 1);
    } else {
        return STATUS_INVALID_BUFFER_SIZE;
    }
    if (c.abi != HXDRV_ABI_VERSION) return STATUS_REVISION_MISMATCH;
    EnterCriticalSection(&p->lock);
    p->state.report_id = HXPAD_REPORT_GAMEPAD;
    p->state.buttons = c.buttons;
    p->state.hat = c.hat > 8 ? 8 : c.hat;
    p->state.lx = c.lx;
    p->state.ly = c.ly;
    p->state.rx = c.rx;
    p->state.ry = c.ry;
    p->state.lt = c.lt;
    p->state.rt = c.rt;
    p->changed = true;
    HxpPump(p);
    LeaveCriticalSection(&p->lock);
    *info = len;
    return STATUS_SUCCESS;
}

static void HxpIoctl(WDFQUEUE queue, WDFREQUEST req, size_t outLen, size_t inLen, ULONG code)
{
    UNREFERENCED_PARAMETER(outLen);
    UNREFERENCED_PARAMETER(inLen);
    HxPad *p = HxGetPad(WdfIoQueueGetDevice(queue));
    NTSTATUS st = STATUS_NOT_SUPPORTED;
    ULONG_PTR info = 0;

    switch (code) {
    case HXP_IOCTL_GET_DEVICE_DESCRIPTOR: {
        HxpHidDescriptor d;
        d.bLength = sizeof(d);
        d.bDescriptorType = 0x21;
        d.bcdHID = 0x0111;
        d.bCountry = 0;
        d.bNumDescriptors = 1;
        d.bReportType = 0x22;
        d.wReportLength = sizeof(kReportDescriptor);
        st = HxpOut(req, &d, sizeof(d), &info);
        break;
    }
    case HXP_IOCTL_GET_REPORT_DESCRIPTOR:
        st = HxpOut(req, kReportDescriptor, sizeof(kReportDescriptor), &info);
        break;
    case HXP_IOCTL_GET_DEVICE_ATTRIBUTES: {
        HxpAttributes a;
        HxpZero(&a, sizeof(a));
        a.Size = sizeof(a);
        a.VendorID = p->vid;
        a.ProductID = p->pid;
        a.VersionNumber = p->version;
        st = HxpOut(req, &a, sizeof(a), &info);
        break;
    }
    case HXP_IOCTL_READ_REPORT: {
        /* pend; completed when the state changes */
        st = WdfRequestForwardToIoQueue(req, p->reads);
        if (NT_SUCCESS(st)) {
            EnterCriticalSection(&p->lock);
            HxpPump(p);
            LeaveCriticalSection(&p->lock);
            return;
        }
        break;
    }
    case HXP_IOCTL_UMDF_GET_INPUT_REPORT: {
        HXPAD_INPUT_REPORT s;
        EnterCriticalSection(&p->lock);
        HxpCopy(&s, &p->state, sizeof(s));
        LeaveCriticalSection(&p->lock);
        st = HxpOut(req, &s, sizeof(s), &info);
        break;
    }
    case HXP_IOCTL_UMDF_SET_OUTPUT_REPORT:
    case HXP_IOCTL_WRITE_REPORT:
        st = HxpControl(p, req, &info);
        break;
    case HXP_IOCTL_GET_STRING: {
        PVOID in;
        size_t inl;
        ULONG id = HXP_STRING_PRODUCT;
        if (NT_SUCCESS(WdfRequestRetrieveInputBuffer(req, sizeof(ULONG), &in, &inl)))
            id = *(ULONG *)in & 0xffff;
        const WCHAR *s = id == HXP_STRING_MANUFACTURER ? p->manufacturer
                         : id == HXP_STRING_SERIAL     ? p->serial
                                                       : p->product;
        st = HxpOut(req, s, (HxpLen(s) + 1) * sizeof(WCHAR), &info);
        break;
    }
    case HXP_IOCTL_ACTIVATE_DEVICE:
    case HXP_IOCTL_DEACTIVATE_DEVICE:
        st = STATUS_SUCCESS;
        break;
    default:
        break;
    }
    WdfRequestCompleteWithInformation(req, st, info);
}

static NTSTATUS HxpDeviceAdd(WDFDRIVER driver, PWDFDEVICE_INIT init)
{
    UNREFERENCED_PARAMETER(driver);
    WDF_OBJECT_ATTRIBUTES attr;
    WDFDEVICE device;
    WDF_IO_QUEUE_CONFIG qc;
    WDFQUEUE q;
    NTSTATUS st;

    /* lower filter under MsHidUmdf.sys (the HID pass-through) */
    HXT("deviceadd", 0);
    WdfFdoInitSetFilter(init);
    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attr, HxPad);
    st = WdfDeviceCreate(&init, &attr, &device);
    HXT("devicecreate", st);
    if (!NT_SUCCESS(st)) return st;
    HxPad *p = HxGetPad(device);
    HxpZero(p, sizeof(*p));
    p->wdf = device;
    InitializeCriticalSection(&p->lock);
    p->state.report_id = HXPAD_REPORT_GAMEPAD;
    p->state.hat = 8;
    HxpReadSettings(p);
    HxpReadSerial(p);

    WDF_IO_QUEUE_CONFIG_INIT_DEFAULT_QUEUE(&qc, WdfIoQueueDispatchParallel);
    /* MsHidUmdf.sys hands the HID class IOCTLs to UMDF as device-control
     * requests; a UMDF queue refuses an internal-device-control callback
     * (WdfIoQueueCreate: STATUS_INVALID_PARAMETER). */
    qc.EvtIoDeviceControl = HxpIoctl;
    st = WdfIoQueueCreate(device, &qc, WDF_NO_OBJECT_ATTRIBUTES, &q);
    HXT("defaultqueue", st);
    if (!NT_SUCCESS(st)) return st;

    WDF_IO_QUEUE_CONFIG_INIT(&qc, WdfIoQueueDispatchManual);
    st = WdfIoQueueCreate(device, &qc, WDF_NO_OBJECT_ATTRIBUTES, &p->reads);
    HXT("readqueue", st);
    return st;
}

extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT obj, PUNICODE_STRING path)
{
    WDF_DRIVER_CONFIG cfg;
    WDF_DRIVER_CONFIG_INIT(&cfg, HxpDeviceAdd);
    NTSTATUS st = WdfDriverCreate(obj, path, WDF_NO_OBJECT_ATTRIBUTES, &cfg, WDF_NO_HANDLE);
    HXT("drivercreate", st);
    return st;
}
