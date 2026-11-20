/*
 * hxidd - Horcrux virtual display: an IddCx indirect display driver (UMDF 2).
 *
 * Gives a Windows guest monitors that exist only in software, at any size
 * and refresh rate the Horcrux guest agent asks for (e.g. 2560x1440 at
 * 240 Hz), so a guest whose GPU has no monitor or no outputs at all still
 * has a desktop to render into. The agent adds and removes monitors through
 * the HXIDD_IOCTL_* control interface (include/hxdrv.h); the last set can be
 * persisted so monitors are back at boot and on the sign-in screen.
 *
 * Frames: the OS renders each monitor's desktop into an IddCx swap chain.
 * The driver consumes every frame (required by IddCx) and, when the agent
 * asks for it (HXIDD_MON_SHARE_FRAMES), copies the frame on the GPU into a
 * named shared D3D11 texture the agent opens on the same adapter - an
 * alternative to DXGI Desktop Duplication with no CPU copy in the driver.
 *
 * Clean-room: written from Microsoft's IddCx/WDF documentation and the
 * WDK headers' declarations only (no Windows-driver-samples code).
 * SPDX-License-Identifier: GPL-3.0-or-later
 */
#define WIN32_LEAN_AND_MEAN
#define UMDF_USING_NTSTATUS
#include <windows.h>
#include <ntstatus.h>
#include <sddl.h>
#include <avrt.h>
#include <dxgi1_5.h>
#include <d3d11_4.h>
#include <wdf.h>
#include <IddCx.h>

#include "hxdrv.h"

#define HXIDD_VERSION ((1u << 16) | 0u)
/* Built-in defaults; each is overridable in the device's hardware key
 * (INF AddReg or `horcrux-guest-agent drivers install`). */
#define HXIDD_DEFAULT_MAX_MONITORS 4u
/* Timeout for a frame-share keyed-mutex acquire: never stall the swap chain. */
#define HXIDD_DEFAULT_SHARE_ACQUIRE_MS 0u

static const GUID HxIddInterface = HXIDD_INTERFACE_GUID_INIT;
/* Base of the per-monitor container ids (last byte = monitor id). */
static const GUID HxIddContainerBase = {
    0x4f1c2b7a, 0x93d5, 0x4e18, { 0xa6, 0x2c, 0x71, 0x0e, 0x5b, 0x3d, 0x98, 0x00 } };

struct HxMon;

struct HxSwap {
    IDDCX_SWAPCHAIN sc;
    HANDLE next;        /* IddCx: next surface available */
    HANDLE stop;        /* ours: stop the thread */
    HANDLE thread;
    DWORD threadId;
    LUID luid;
    HxMon *mon;
    ID3D11Device *dev;
    ID3D11DeviceContext *ctx;
    /* frame sharing */
    ID3D11Texture2D *shared;
    IDXGIKeyedMutex *mutex;
    HANDLE sharedHandle;
    HANDLE frameEvent;
    UINT sharedW, sharedH;
    DXGI_FORMAT sharedFmt;
};

struct HxMon {
    bool used;
    IDDCX_MONITOR obj;     /* NULL until created */
    HXIDD_MONITOR want;    /* what the agent asked for */
    UINT osTargetId;
    volatile LONG64 frames;
    volatile LONG shareGen;
    volatile LONG lastError;
    UINT curW, curH, curMhz;
    HxSwap *swap;
};

struct HxDev {
    WDFDEVICE wdf;
    IDDCX_ADAPTER adapter;
    bool ready;
    LUID adapterLuid;
    UINT maxMonitors;
    UINT shareAcquireMs;
    SRWLOCK lock;          /* short sections over mons[]; never held across IddCx calls */
    CRITICAL_SECTION ctl;  /* serialises monitor plug/unplug (IOCTLs, adapter start) */
    HxMon mons[HXIDD_ABI_MONITORS];
};

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(HxDev, HxGetDev);

/* One device per driver host process (UmdfHostProcessSharing disabled in
 * the INF), so IddCx callbacks that carry only an adapter/monitor handle
 * find their device here. */
static HxDev *g_dev;

extern "C" DRIVER_INITIALIZE DriverEntry;
static EVT_WDF_DRIVER_DEVICE_ADD HxDeviceAdd;
static EVT_WDF_DEVICE_D0_ENTRY HxD0Entry;
static EVT_IDD_CX_DEVICE_IO_CONTROL HxIoctl;
static EVT_IDD_CX_ADAPTER_INIT_FINISHED HxAdapterInitFinished;
static EVT_IDD_CX_ADAPTER_COMMIT_MODES HxCommitModes;
static EVT_IDD_CX_PARSE_MONITOR_DESCRIPTION HxParseDescription;
static EVT_IDD_CX_MONITOR_GET_DEFAULT_DESCRIPTION_MODES HxDefaultModes;
static EVT_IDD_CX_MONITOR_QUERY_TARGET_MODES HxTargetModes;
static EVT_IDD_CX_MONITOR_ASSIGN_SWAPCHAIN HxAssignSwapChain;
static EVT_IDD_CX_MONITOR_UNASSIGN_SWAPCHAIN HxUnassignSwapChain;

/* ---- helpers ------------------------------------------------------------ */

static void HxZero(void *p, size_t n)
{
    volatile unsigned char *b = (volatile unsigned char *)p;
    while (n--) *b++ = 0;
}

static void HxCopy(void *d, const void *s, size_t n)
{
    unsigned char *a = (unsigned char *)d;
    const unsigned char *b = (const unsigned char *)s;
    while (n--) *a++ = *b++;
}

static bool HxSame(const void *a, const void *b, size_t n)
{
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    while (n--)
        if (*x++ != *y++) return false;
    return true;
}

static HxMon *HxFind(HxDev *d, IDDCX_MONITOR m)
{
    for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++)
        if (d->mons[i].used && d->mons[i].obj == m) return &d->mons[i];
    return NULL;
}

static ULONG HxRegDword(WDFKEY key, PCWSTR name, ULONG def)
{
    UNICODE_STRING n;
    ULONG v = def;
    n.Buffer = (PWSTR)name;
    n.Length = (USHORT)(lstrlenW(name) * sizeof(WCHAR));
    n.MaximumLength = n.Length;
    if (!NT_SUCCESS(WdfRegistryQueryULong(key, &n, &v))) v = def;
    return v;
}

static void HxReadSettings(HxDev *d)
{
    WDFKEY key;
    d->maxMonitors = HXIDD_DEFAULT_MAX_MONITORS;
    d->shareAcquireMs = HXIDD_DEFAULT_SHARE_ACQUIRE_MS;
    if (NT_SUCCESS(WdfDeviceOpenRegistryKey(d->wdf, PLUGPLAY_REGKEY_DEVICE, KEY_READ,
                                            WDF_NO_OBJECT_ATTRIBUTES, &key))) {
        d->maxMonitors = HxRegDword(key, L"MaxMonitors", d->maxMonitors);
        d->shareAcquireMs = HxRegDword(key, L"ShareAcquireMs", d->shareAcquireMs);
        WdfRegistryClose(key);
    }
    if (d->maxMonitors < 1) d->maxMonitors = 1;
    if (d->maxMonitors > HXIDD_ABI_MONITORS) d->maxMonitors = HXIDD_ABI_MONITORS;
}

/* The persisted monitor set lives in the driver's own device subkey, the
 * only registry location a UMDF driver may write. */
static const WCHAR kPersistName[] = L"Monitors";

static void HxPersist(HxDev *d, const HXIDD_SET_MONITORS *set)
{
    WDFKEY key;
    UNICODE_STRING n;
    n.Buffer = (PWSTR)kPersistName;
    n.Length = (USHORT)(sizeof(kPersistName) - sizeof(WCHAR));
    n.MaximumLength = n.Length;
    if (!NT_SUCCESS(WdfDeviceOpenRegistryKey(d->wdf, PLUGPLAY_REGKEY_DEVICE | WDF_REGKEY_DEVICE_SUBKEY,
                                             KEY_READ | KEY_SET_VALUE, WDF_NO_OBJECT_ATTRIBUTES, &key)))
        return;
    if (set && (set->flags & HXIDD_SET_PERSIST))
        WdfRegistryAssignValue(key, &n, REG_BINARY, sizeof(*set), (PVOID)set);
    else
        WdfRegistryRemoveValue(key, &n);
    WdfRegistryClose(key);
}

static bool HxLoadPersisted(HxDev *d, HXIDD_SET_MONITORS *set)
{
    WDFKEY key;
    UNICODE_STRING n;
    ULONG got = 0, type = 0;
    bool ok = false;
    n.Buffer = (PWSTR)kPersistName;
    n.Length = (USHORT)(sizeof(kPersistName) - sizeof(WCHAR));
    n.MaximumLength = n.Length;
    if (!NT_SUCCESS(WdfDeviceOpenRegistryKey(d->wdf, PLUGPLAY_REGKEY_DEVICE | WDF_REGKEY_DEVICE_SUBKEY,
                                             KEY_READ, WDF_NO_OBJECT_ATTRIBUTES, &key)))
        return false;
    if (NT_SUCCESS(WdfRegistryQueryValue(key, &n, sizeof(*set), set, &got, &type)) &&
        type == REG_BINARY && got == sizeof(*set) && set->abi == HXDRV_ABI_VERSION)
        ok = true;
    WdfRegistryClose(key);
    return ok;
}

/* DISPLAYCONFIG signal for a mode. The virtual link has no blanking, so the
 * total size equals the active size; rates are exact rationals. */
static void HxSignal(DISPLAYCONFIG_VIDEO_SIGNAL_INFO *s, const HXIDD_MODE *m, UINT divider)
{
    HxZero(s, sizeof(*s));
    s->totalSize.cx = s->activeSize.cx = m->width;
    s->totalSize.cy = s->activeSize.cy = m->height;
    s->vSyncFreq.Numerator = m->refresh_mhz;
    s->vSyncFreq.Denominator = 1000;
    s->hSyncFreq.Numerator = m->refresh_mhz * m->height;
    s->hSyncFreq.Denominator = 1000;
    s->pixelRate = (UINT64)m->refresh_mhz * m->width * m->height / 1000;
    s->AdditionalSignalInfo.videoStandard = 255; /* D3DKMDT_VSS_OTHER */
    s->AdditionalSignalInfo.vSyncFreqDivider = divider;
    s->scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
}

static bool HxModeValid(const HXIDD_MODE *m)
{
    /* Bounds of what DXGI/D3D11 textures can hold; refresh up to 1000 Hz. */
    return m->width >= 320 && m->height >= 200 && m->width <= 16384 && m->height <= 16384 &&
           m->refresh_mhz >= 1000 && m->refresh_mhz <= 1000000;
}

/* ---- monitors ----------------------------------------------------------- */

/* Create and announce a monitor for slot m (caller holds ctl, not lock):
 * IddCx calls back into HxDefaultModes/HxTargetModes during arrival. */
static NTSTATUS HxPlug(HxDev *d, HxMon *m)
{
    IDDCX_MONITOR_INFO info;
    IDARG_IN_MONITORCREATE in;
    IDARG_OUT_MONITORCREATE out;
    IDARG_OUT_MONITORARRIVAL arr;
    WDF_OBJECT_ATTRIBUTES attr;
    NTSTATUS st;

    HxZero(&info, sizeof(info));
    info.Size = sizeof(info);
    info.MonitorType = DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED;
    info.ConnectorIndex = m->want.id;
    info.MonitorDescription.Size = sizeof(info.MonitorDescription);
    /* No EDID: the OS asks HxDefaultModes for the modes instead. */
    info.MonitorDescription.Type = IDDCX_MONITOR_DESCRIPTION_TYPE_EDID;
    info.MonitorDescription.DataSize = 0;
    info.MonitorDescription.pData = NULL;
    info.MonitorContainerId = HxIddContainerBase;
    info.MonitorContainerId.Data4[7] = (UCHAR)m->want.id;

    WDF_OBJECT_ATTRIBUTES_INIT(&attr);
    HxZero(&in, sizeof(in));
    in.ObjectAttributes = &attr;
    in.pMonitorInfo = &info;
    st = IddCxMonitorCreate(d->adapter, &in, &out);
    if (!NT_SUCCESS(st)) return st;
    AcquireSRWLockExclusive(&d->lock);
    m->obj = out.MonitorObject;
    ReleaseSRWLockExclusive(&d->lock);
    st = IddCxMonitorArrival(out.MonitorObject, &arr);
    AcquireSRWLockExclusive(&d->lock);
    if (NT_SUCCESS(st)) {
        m->osTargetId = arr.OsTargetId;
        d->adapterLuid = arr.OsAdapterLuid;
    } else {
        m->obj = NULL;
    }
    ReleaseSRWLockExclusive(&d->lock);
    if (!NT_SUCCESS(st)) WdfObjectDelete((WDFOBJECT)out.MonitorObject);
    return st;
}

/* Remove slot m's monitor (caller holds ctl, not lock). The slot keeps its
 * handle during departure so the unassign callback still finds it. */
static void HxSwapStop(HxMon *m);

static void HxUnplug(HxDev *d, HxMon *m)
{
    /* Stop the frame thread first: after the departure its swap chain is
     * gone, and the unassign callback may come after this slot is reused. */
    HxMon tmp;
    HxZero(&tmp, sizeof(tmp));
    AcquireSRWLockExclusive(&d->lock);
    tmp.swap = m->swap;
    m->swap = NULL;
    ReleaseSRWLockExclusive(&d->lock);
    HxSwapStop(&tmp);
    if (m->obj) IddCxMonitorDeparture(m->obj);
    AcquireSRWLockExclusive(&d->lock);
    m->obj = NULL;
    m->curW = m->curH = m->curMhz = 0;
    m->used = false;
    ReleaseSRWLockExclusive(&d->lock);
}

static bool HxWantValid(const HXIDD_MONITOR *w)
{
    if (w->mode_count < 1 || w->mode_count > HXIDD_ABI_MODES) return false;
    for (UINT i = 0; i < w->mode_count; i++)
        if (!HxModeValid(&w->modes[i])) return false;
    return w->id < 256;
}

/* Make the plugged monitors match `set` (caller holds ctl). Monitors that
 * are unchanged stay plugged; changed ones are re-plugged. */
static NTSTATUS HxApply(HxDev *d, const HXIDD_SET_MONITORS *set)
{
    NTSTATUS result = STATUS_SUCCESS;
    if (set->count > d->maxMonitors) return STATUS_INVALID_PARAMETER;
    for (UINT i = 0; i < set->count; i++) {
        if (!HxWantValid(&set->monitors[i])) return STATUS_INVALID_PARAMETER;
        for (UINT j = 0; j < i; j++)
            if (set->monitors[j].id == set->monitors[i].id) return STATUS_INVALID_PARAMETER;
    }

    for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++) {
        HxMon *m = &d->mons[i];
        if (!m->used) continue;
        bool keep = false;
        for (UINT j = 0; j < set->count; j++)
            if (HxSame(&set->monitors[j], &m->want, sizeof(m->want))) keep = true;
        if (!keep) {
            if (m->obj) {
                HxUnplug(d, m);
            } else {
                AcquireSRWLockExclusive(&d->lock);
                m->used = false;
                ReleaseSRWLockExclusive(&d->lock);
            }
        }
    }
    for (UINT j = 0; j < set->count; j++) {
        HxMon *m = NULL;
        for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++)
            if (d->mons[i].used && d->mons[i].want.id == set->monitors[j].id) m = &d->mons[i];
        if (m) continue; /* unchanged, still plugged */
        AcquireSRWLockExclusive(&d->lock);
        for (UINT i = 0; i < HXIDD_ABI_MONITORS && !m; i++)
            if (!d->mons[i].used) m = &d->mons[i];
        if (m) {
            HxZero(m, sizeof(*m));
            m->used = true;
            HxCopy(&m->want, &set->monitors[j], sizeof(m->want));
        }
        ReleaseSRWLockExclusive(&d->lock);
        if (!m) return STATUS_INSUFFICIENT_RESOURCES;
        if (d->ready) {
            NTSTATUS st = HxPlug(d, m);
            if (!NT_SUCCESS(st)) {
                AcquireSRWLockExclusive(&d->lock);
                m->used = false;
                ReleaseSRWLockExclusive(&d->lock);
                result = st;
            }
        }
    }
    return result;
}

/* ---- swap chain --------------------------------------------------------- */

static void HxShareRelease(HxSwap *s)
{
    if (s->mutex) s->mutex->Release();
    if (s->shared) s->shared->Release();
    if (s->sharedHandle) CloseHandle(s->sharedHandle);
    s->mutex = NULL;
    s->shared = NULL;
    s->sharedHandle = NULL;
    s->sharedW = s->sharedH = 0;
}

/* SY, administrators, interactive users (the agent runs in the user's
 * session) and LocalService (the driver host). */
static const WCHAR kShareSddl[] = L"D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;IU)(A;;GA;;;LS)";

static void HxName(WCHAR *buf, size_t cap, UINT id, const WCHAR *tail, LONG gen, bool withGen)
{
    /* "Global\HorcruxIdd" + id + tail [+ gen], small fixed formatter */
    const WCHAR *p = HXIDD_SHARED_PREFIX;
    size_t n = 0;
    while (*p && n + 1 < cap) buf[n++] = *p++;
    WCHAR tmp[16];
    int t = 0;
    UINT v = id;
    do { tmp[t++] = (WCHAR)(L'0' + v % 10); v /= 10; } while (v && t < 15);
    while (t && n + 1 < cap) buf[n++] = tmp[--t];
    while (*tail && n + 1 < cap) buf[n++] = *tail++;
    if (withGen) {
        ULONG g = (ULONG)gen;
        do { tmp[t++] = (WCHAR)(L'0' + g % 10); g /= 10; } while (g && t < 15);
        while (t && n + 1 < cap) buf[n++] = tmp[--t];
    }
    buf[n] = 0;
}

static HRESULT HxShareEnsure(HxSwap *s, ID3D11Texture2D *src)
{
    D3D11_TEXTURE2D_DESC sd, td;
    src->GetDesc(&sd);
    if (s->shared && s->sharedW == sd.Width && s->sharedH == sd.Height && s->sharedFmt == sd.Format)
        return S_OK;
    HxShareRelease(s);

    SECURITY_ATTRIBUTES sa;
    PSECURITY_DESCRIPTOR sec = NULL;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(kShareSddl, SDDL_REVISION_1, &sec, NULL))
        return HRESULT_FROM_WIN32(GetLastError());
    sa.nLength = sizeof(sa);
    sa.lpSecurityDescriptor = sec;
    sa.bInheritHandle = FALSE;

    HRESULT hr = S_OK;
    if (!s->frameEvent) {
        WCHAR en[96];
        HxName(en, 96, s->mon->want.id, L"Frame", 0, false);
        s->frameEvent = CreateEventW(&sa, FALSE, FALSE, en);
        if (!s->frameEvent) hr = HRESULT_FROM_WIN32(GetLastError());
    }
    if (SUCCEEDED(hr)) {
        HxZero(&td, sizeof(td));
        td.Width = sd.Width;
        td.Height = sd.Height;
        td.MipLevels = 1;
        td.ArraySize = 1;
        td.Format = sd.Format;
        td.SampleDesc.Count = 1;
        td.Usage = D3D11_USAGE_DEFAULT;
        td.BindFlags = D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET;
        td.MiscFlags = D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX;
        hr = s->dev->CreateTexture2D(&td, NULL, &s->shared);
    }
    if (SUCCEEDED(hr)) hr = s->shared->QueryInterface(__uuidof(IDXGIKeyedMutex), (void **)&s->mutex);
    if (SUCCEEDED(hr)) {
        IDXGIResource1 *r = NULL;
        hr = s->shared->QueryInterface(__uuidof(IDXGIResource1), (void **)&r);
        if (SUCCEEDED(hr)) {
            LONG gen = InterlockedIncrement(&s->mon->shareGen);
            WCHAR tn[96];
            HxName(tn, 96, s->mon->want.id, L"Frame", gen, true);
            hr = r->CreateSharedHandle(&sa, DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE, tn,
                                       &s->sharedHandle);
            r->Release();
        }
    }
    LocalFree(sec);
    if (FAILED(hr)) {
        HxShareRelease(s);
        return hr;
    }
    s->sharedW = sd.Width;
    s->sharedH = sd.Height;
    s->sharedFmt = sd.Format;
    return S_OK;
}

static void HxShareFrame(HxSwap *s, IDXGIResource *surface, UINT acquireMs)
{
    ID3D11Texture2D *src = NULL;
    if (FAILED(surface->QueryInterface(__uuidof(ID3D11Texture2D), (void **)&src))) return;
    HRESULT hr = HxShareEnsure(s, src);
    if (SUCCEEDED(hr)) {
        /* Key 0 is used as a plain mutex by both sides. If the agent still
         * holds the texture the frame is skipped, never waited for. */
        hr = s->mutex->AcquireSync(0, acquireMs);
        if (hr == S_OK) {
            s->ctx->CopyResource(s->shared, src);
            s->ctx->Flush();
            s->mutex->ReleaseSync(0);
            SetEvent(s->frameEvent);
        }
    } else {
        InterlockedExchange(&s->mon->lastError, hr);
    }
    src->Release();
}

static DWORD WINAPI HxSwapThread(LPVOID arg)
{
    HxSwap *s = (HxSwap *)arg;
    DWORD avIndex = 0;
    HANDLE av = AvSetMmThreadCharacteristicsW(L"Distribution", &avIndex);
    bool lost = false;

    IDXGIDevice *dxgi = NULL;
    HRESULT hr = s->dev->QueryInterface(__uuidof(IDXGIDevice), (void **)&dxgi);
    if (SUCCEEDED(hr)) {
        IDARG_IN_SWAPCHAINSETDEVICE sd;
        sd.pDevice = dxgi;
        hr = IddCxSwapChainSetDevice(s->sc, &sd);
        dxgi->Release();
    }
    if (FAILED(hr)) {
        InterlockedExchange(&s->mon->lastError, hr);
        lost = true;
    }
    while (!lost) {
        IDARG_OUT_RELEASEANDACQUIREBUFFER buf;
        HxZero(&buf, sizeof(buf));
        hr = IddCxSwapChainReleaseAndAcquireBuffer(s->sc, &buf);
        if (hr == E_PENDING) {
            HANDLE w[2] = { s->next, s->stop };
            DWORD r = WaitForMultipleObjects(2, w, FALSE, INFINITE);
            if (r == WAIT_OBJECT_0) continue;
            break; /* stop requested */
        }
        if (FAILED(hr)) {
            InterlockedExchange(&s->mon->lastError, hr);
            lost = true;
            break;
        }
        if (buf.MetaData.pSurface) {
            if (s->mon->want.flags & HXIDD_MON_SHARE_FRAMES)
                HxShareFrame(s, buf.MetaData.pSurface, g_dev ? g_dev->shareAcquireMs : 0);
            buf.MetaData.pSurface->Release();
        }
        IddCxSwapChainFinishedProcessingFrame(s->sc);
        InterlockedIncrement64(&s->mon->frames);
        if (WaitForSingleObject(s->stop, 0) == WAIT_OBJECT_0) break;
    }
    if (av) AvRevertMmThreadCharacteristics(av);
    if (lost) {
        /* The swap chain is unusable (device lost, mode change): deleting it
         * makes the OS assign a new one. */
        WdfObjectDelete((WDFOBJECT)s->sc);
    }
    return 0;
}

static void HxSwapFree(HxSwap *s)
{
    HxShareRelease(s);
    if (s->frameEvent) CloseHandle(s->frameEvent);
    if (s->ctx) s->ctx->Release();
    if (s->dev) s->dev->Release();
    if (s->stop) CloseHandle(s->stop);
    if (s->thread) CloseHandle(s->thread);
    HeapFree(GetProcessHeap(), 0, s);
}

static void HxSwapStop(HxMon *m)
{
    HxSwap *s = m->swap;
    if (!s) return;
    m->swap = NULL;
    SetEvent(s->stop);
    /* never wait for ourselves (an IddCx callback on the swap thread) */
    if (s->thread && GetCurrentThreadId() != s->threadId) WaitForSingleObject(s->thread, INFINITE);
    HxSwapFree(s);
}

static HRESULT HxCreateDevice(LUID luid, ID3D11Device **dev, ID3D11DeviceContext **ctx)
{
    IDXGIFactory5 *f = NULL;
    IDXGIAdapter1 *a = NULL;
    HRESULT hr = CreateDXGIFactory2(0, __uuidof(IDXGIFactory5), (void **)&f);
    if (SUCCEEDED(hr)) hr = f->EnumAdapterByLuid(luid, __uuidof(IDXGIAdapter1), (void **)&a);
    if (SUCCEEDED(hr))
        hr = D3D11CreateDevice(a, D3D_DRIVER_TYPE_UNKNOWN, NULL, D3D11_CREATE_DEVICE_BGRA_SUPPORT, NULL, 0,
                               D3D11_SDK_VERSION, dev, NULL, ctx);
    if (a) a->Release();
    if (f) f->Release();
    return hr;
}

/* ---- IddCx callbacks ---------------------------------------------------- */

static NTSTATUS HxAdapterInitFinished(IDDCX_ADAPTER adapter, const IDARG_IN_ADAPTER_INIT_FINISHED *in)
{
    HxDev *d = g_dev;
    if (!d) return STATUS_DEVICE_NOT_READY;
    if (!NT_SUCCESS(in->AdapterInitStatus)) return STATUS_SUCCESS;
    EnterCriticalSection(&d->ctl);
    AcquireSRWLockExclusive(&d->lock);
    d->adapter = adapter;
    d->ready = true;
    bool pending = false;
    for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++)
        if (d->mons[i].used && !d->mons[i].obj) pending = true;
    ReleaseSRWLockExclusive(&d->lock);
    if (pending) {
        /* monitors the agent asked for before the adapter was ready */
        for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++)
            if (d->mons[i].used && !d->mons[i].obj && !NT_SUCCESS(HxPlug(d, &d->mons[i]))) {
                AcquireSRWLockExclusive(&d->lock);
                d->mons[i].used = false;
                ReleaseSRWLockExclusive(&d->lock);
            }
    } else {
        HXIDD_SET_MONITORS *saved = (HXIDD_SET_MONITORS *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY,
                                                                    sizeof(HXIDD_SET_MONITORS));
        if (saved) {
            if (HxLoadPersisted(d, saved)) HxApply(d, saved);
            HeapFree(GetProcessHeap(), 0, saved);
        }
    }
    LeaveCriticalSection(&d->ctl);
    return STATUS_SUCCESS;
}

static NTSTATUS HxCommitModes(IDDCX_ADAPTER adapter, const IDARG_IN_COMMITMODES *in)
{
    UNREFERENCED_PARAMETER(adapter);
    HxDev *d = g_dev;
    if (!d) return STATUS_SUCCESS;
    AcquireSRWLockExclusive(&d->lock);
    for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++) {
        d->mons[i].curW = d->mons[i].curH = d->mons[i].curMhz = 0;
    }
    for (UINT p = 0; p < in->PathCount; p++) {
        const IDDCX_PATH *path = &in->pPaths[p];
        HxMon *m = HxFind(d, path->MonitorObject);
        if (!m || !(path->Flags & IDDCX_PATH_FLAGS_ACTIVE)) continue;
        const DISPLAYCONFIG_VIDEO_SIGNAL_INFO *s = &path->TargetVideoSignalInfo;
        m->curW = s->activeSize.cx;
        m->curH = s->activeSize.cy;
        m->curMhz = s->vSyncFreq.Denominator
                        ? (UINT)((UINT64)s->vSyncFreq.Numerator * 1000 / s->vSyncFreq.Denominator)
                        : 0;
    }
    ReleaseSRWLockExclusive(&d->lock);
    return STATUS_SUCCESS;
}

static NTSTATUS HxParseDescription(const IDARG_IN_PARSEMONITORDESCRIPTION *in,
                                   IDARG_OUT_PARSEMONITORDESCRIPTION *out)
{
    /* hxidd monitors carry no EDID, so this is never asked for. */
    UNREFERENCED_PARAMETER(in);
    out->MonitorModeBufferOutputCount = 0;
    out->PreferredMonitorModeIdx = 0;
    return STATUS_INVALID_PARAMETER;
}

static NTSTATUS HxDefaultModes(IDDCX_MONITOR monitor, const IDARG_IN_GETDEFAULTDESCRIPTIONMODES *in,
                               IDARG_OUT_GETDEFAULTDESCRIPTIONMODES *out)
{
    HxDev *d = g_dev;
    if (!d) return STATUS_DEVICE_NOT_READY;
    AcquireSRWLockShared(&d->lock);
    HxMon *m = HxFind(d, monitor);
    NTSTATUS st = STATUS_SUCCESS;
    if (!m) {
        st = STATUS_INVALID_PARAMETER;
    } else {
        UINT n = m->want.mode_count;
        out->DefaultMonitorModeBufferOutputCount = n;
        out->PreferredMonitorModeIdx = 0;
        if (in->DefaultMonitorModeBufferInputCount != 0) {
            if (in->DefaultMonitorModeBufferInputCount < n) {
                st = STATUS_BUFFER_TOO_SMALL;
            } else {
                for (UINT i = 0; i < n; i++) {
                    IDDCX_MONITOR_MODE *mm = &in->pDefaultMonitorModes[i];
                    mm->Size = sizeof(*mm);
                    mm->Origin = IDDCX_MONITOR_MODE_ORIGIN_DRIVER;
                    HxSignal(&mm->MonitorVideoSignalInfo, &m->want.modes[i], 0);
                }
            }
        }
    }
    ReleaseSRWLockShared(&d->lock);
    return st;
}

static NTSTATUS HxTargetModes(IDDCX_MONITOR monitor, const IDARG_IN_QUERYTARGETMODES *in,
                              IDARG_OUT_QUERYTARGETMODES *out)
{
    HxDev *d = g_dev;
    if (!d) return STATUS_DEVICE_NOT_READY;
    AcquireSRWLockShared(&d->lock);
    HxMon *m = HxFind(d, monitor);
    NTSTATUS st = STATUS_SUCCESS;
    if (!m) {
        st = STATUS_INVALID_PARAMETER;
    } else {
        UINT n = m->want.mode_count;
        out->TargetModeBufferOutputCount = n;
        if (in->TargetModeBufferInputCount != 0) {
            if (in->TargetModeBufferInputCount < n) {
                st = STATUS_BUFFER_TOO_SMALL;
            } else {
                for (UINT i = 0; i < n; i++) {
                    IDDCX_TARGET_MODE *t = &in->pTargetModes[i];
                    HxZero(t, sizeof(*t));
                    t->Size = sizeof(*t);
                    HxSignal(&t->TargetVideoSignalInfo.targetVideoSignalInfo, &m->want.modes[i], 1);
                }
            }
        }
    }
    ReleaseSRWLockShared(&d->lock);
    return st;
}

static NTSTATUS HxAssignSwapChain(IDDCX_MONITOR monitor, const IDARG_IN_SETSWAPCHAIN *in)
{
    HxDev *d = g_dev;
    if (!d) return STATUS_DEVICE_NOT_READY;
    HxMon old;
    HxZero(&old, sizeof(old));
    AcquireSRWLockExclusive(&d->lock);
    HxMon *m = HxFind(d, monitor);
    if (m) {
        old.swap = m->swap;
        m->swap = NULL;
    }
    ReleaseSRWLockExclusive(&d->lock);
    if (!m) return STATUS_INVALID_PARAMETER;
    HxSwapStop(&old);

    HxSwap *s = (HxSwap *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, sizeof(HxSwap));
    if (!s) return STATUS_NO_MEMORY;
    s->sc = in->hSwapChain;
    s->next = in->hNextSurfaceAvailable;
    s->luid = in->RenderAdapterLuid;
    s->mon = m;
    s->stop = CreateEventW(NULL, TRUE, FALSE, NULL);
    HRESULT hr = s->stop ? HxCreateDevice(s->luid, &s->dev, &s->ctx) : E_OUTOFMEMORY;
    if (SUCCEEDED(hr)) {
        AcquireSRWLockExclusive(&d->lock);
        m->swap = s;
        ReleaseSRWLockExclusive(&d->lock);
        s->thread = CreateThread(NULL, 0, HxSwapThread, s, 0, &s->threadId);
        if (!s->thread) {
            hr = HRESULT_FROM_WIN32(GetLastError());
            AcquireSRWLockExclusive(&d->lock);
            m->swap = NULL;
            ReleaseSRWLockExclusive(&d->lock);
        }
    }
    if (FAILED(hr)) {
        /* the OS retries with a new swap chain */
        InterlockedExchange(&m->lastError, hr);
        HxSwapFree(s);
        WdfObjectDelete((WDFOBJECT)in->hSwapChain);
        return STATUS_UNSUCCESSFUL;
    }
    return STATUS_SUCCESS;
}

static NTSTATUS HxUnassignSwapChain(IDDCX_MONITOR monitor)
{
    HxDev *d = g_dev;
    if (!d) return STATUS_SUCCESS;
    HxMon tmp;
    HxZero(&tmp, sizeof(tmp));
    AcquireSRWLockExclusive(&d->lock);
    HxMon *m = HxFind(d, monitor);
    if (m) {
        tmp.swap = m->swap; /* stop outside the lock: the thread may call into IddCx */
        m->swap = NULL;
    }
    ReleaseSRWLockExclusive(&d->lock);
    HxSwapStop(&tmp);
    return STATUS_SUCCESS;
}

/* ---- control interface -------------------------------------------------- */

static void HxIoctl(WDFDEVICE device, WDFREQUEST req, size_t outLen, size_t inLen, ULONG code)
{
    UNREFERENCED_PARAMETER(outLen);
    UNREFERENCED_PARAMETER(inLen);
    HxDev *d = HxGetDev(device);
    NTSTATUS st = STATUS_INVALID_DEVICE_REQUEST;
    ULONG_PTR info = 0;

    if (code == HXIDD_IOCTL_GET_INFO) {
        HXIDD_INFO *o = NULL;
        st = WdfRequestRetrieveOutputBuffer(req, sizeof(HXIDD_INFO), (PVOID *)&o, NULL);
        if (NT_SUCCESS(st)) {
            IDARG_OUT_GETVERSION ver;
            HxZero(o, sizeof(*o));
            o->abi = HXDRV_ABI_VERSION;
            o->driver_version = HXIDD_VERSION;
            if (NT_SUCCESS(IddCxGetVersion(&ver))) o->iddcx_version = ver.IddCxVersion;
            AcquireSRWLockShared(&d->lock);
            o->max_monitors = d->maxMonitors;
            o->adapter_ready = d->ready;
            o->adapter_luid_low = d->adapterLuid.LowPart;
            o->adapter_luid_high = d->adapterLuid.HighPart;
            UINT n = 0;
            for (UINT i = 0; i < HXIDD_ABI_MONITORS; i++) {
                HxMon *m = &d->mons[i];
                if (!m->used) continue;
                HXIDD_MONITOR_INFO *mi = &o->monitors[n++];
                mi->id = m->want.id;
                mi->flags = m->want.flags;
                mi->width = m->curW;
                mi->height = m->curH;
                mi->refresh_mhz = m->curMhz;
                mi->os_target_id = m->osTargetId;
                mi->has_swapchain = m->swap != NULL;
                mi->share_generation = (uint32_t)m->shareGen;
                mi->frames = (uint64_t)m->frames;
                if (m->swap) {
                    mi->render_luid_low = m->swap->luid.LowPart;
                    mi->render_luid_high = m->swap->luid.HighPart;
                }
                mi->last_error = (uint32_t)m->lastError;
                mi->pref_width = m->want.modes[0].width;
                mi->pref_height = m->want.modes[0].height;
                mi->pref_refresh_mhz = m->want.modes[0].refresh_mhz;
            }
            o->count = n;
            ReleaseSRWLockShared(&d->lock);
            info = sizeof(HXIDD_INFO);
        }
    } else if (code == HXIDD_IOCTL_SET_MONITORS) {
        HXIDD_SET_MONITORS *set = NULL;
        st = WdfRequestRetrieveInputBuffer(req, sizeof(HXIDD_SET_MONITORS), (PVOID *)&set, NULL);
        if (NT_SUCCESS(st) && set->abi != HXDRV_ABI_VERSION) st = STATUS_REVISION_MISMATCH;
        if (NT_SUCCESS(st)) {
            /* heap copy: the request buffer is only valid until completion
             * and the structure is too big for a driver stack frame */
            HXIDD_SET_MONITORS *copy =
                (HXIDD_SET_MONITORS *)HeapAlloc(GetProcessHeap(), 0, sizeof(HXIDD_SET_MONITORS));
            if (!copy) {
                st = STATUS_NO_MEMORY;
            } else {
                HxCopy(copy, set, sizeof(*copy));
                EnterCriticalSection(&d->ctl);
                st = HxApply(d, copy);
                if (NT_SUCCESS(st)) HxPersist(d, copy);
                LeaveCriticalSection(&d->ctl);
                HeapFree(GetProcessHeap(), 0, copy);
            }
        }
    }
    WdfRequestCompleteWithInformation(req, st, info);
}

/* ---- WDF ---------------------------------------------------------------- */

static NTSTATUS HxD0Entry(WDFDEVICE device, WDF_POWER_DEVICE_STATE prev)
{
    UNREFERENCED_PARAMETER(prev);
    HxDev *d = HxGetDev(device);
    if (d->adapter) return STATUS_SUCCESS; /* resume: adapter already exists */

    IDDCX_ADAPTER_CAPS caps;
    IDARG_IN_ADAPTER_INIT in;
    IDARG_OUT_ADAPTER_INIT out;
    WDF_OBJECT_ATTRIBUTES attr;
    IDDCX_ENDPOINT_VERSION ver;

    HxZero(&ver, sizeof(ver));
    ver.Size = sizeof(ver);
    ver.MajorVer = HXIDD_VERSION >> 16;
    ver.MinorVer = HXIDD_VERSION & 0xffff;

    HxZero(&caps, sizeof(caps));
    caps.Size = sizeof(caps);
    caps.Flags = IDDCX_ADAPTER_FLAGS_NONE;
    caps.MaxMonitorsSupported = d->maxMonitors;
    caps.EndPointDiagnostics.Size = sizeof(caps.EndPointDiagnostics);
    caps.EndPointDiagnostics.GammaSupport = IDDCX_FEATURE_IMPLEMENTATION_NONE;
    caps.EndPointDiagnostics.TransmissionType = IDDCX_TRANSMISSION_TYPE_OTHER;
    caps.EndPointDiagnostics.pEndPointFriendlyName = L"Horcrux Virtual Display";
    caps.EndPointDiagnostics.pEndPointManufacturerName = L"Horcrux";
    caps.EndPointDiagnostics.pEndPointModelName = L"hxidd";
    caps.EndPointDiagnostics.pHardwareVersion = &ver;
    caps.EndPointDiagnostics.pFirmwareVersion = &ver;

    WDF_OBJECT_ATTRIBUTES_INIT(&attr);
    HxZero(&in, sizeof(in));
    in.WdfDevice = device;
    in.pCaps = &caps;
    in.ObjectAttributes = &attr;
    NTSTATUS st = IddCxAdapterInitAsync(&in, &out);
    if (NT_SUCCESS(st)) d->adapter = out.AdapterObject;
    return st;
}

static NTSTATUS HxDeviceAdd(WDFDRIVER driver, PWDFDEVICE_INIT init)
{
    UNREFERENCED_PARAMETER(driver);
    WDF_PNPPOWER_EVENT_CALLBACKS pnp;
    IDD_CX_CLIENT_CONFIG cfg;
    WDF_OBJECT_ATTRIBUTES attr;
    WDFDEVICE device;
    NTSTATUS st;

    WDF_PNPPOWER_EVENT_CALLBACKS_INIT(&pnp);
    pnp.EvtDeviceD0Entry = HxD0Entry;
    WdfDeviceInitSetPnpPowerEventCallbacks(init, &pnp);

    IDD_CX_CLIENT_CONFIG_INIT(&cfg);
    cfg.EvtIddCxDeviceIoControl = HxIoctl;
    cfg.EvtIddCxAdapterInitFinished = HxAdapterInitFinished;
    cfg.EvtIddCxParseMonitorDescription = HxParseDescription;
    cfg.EvtIddCxMonitorGetDefaultDescriptionModes = HxDefaultModes;
    cfg.EvtIddCxMonitorQueryTargetModes = HxTargetModes;
    cfg.EvtIddCxAdapterCommitModes = HxCommitModes;
    cfg.EvtIddCxMonitorAssignSwapChain = HxAssignSwapChain;
    cfg.EvtIddCxMonitorUnassignSwapChain = HxUnassignSwapChain;
    st = IddCxDeviceInitConfig(init, &cfg);
    if (!NT_SUCCESS(st)) return st;

    WDF_OBJECT_ATTRIBUTES_INIT_CONTEXT_TYPE(&attr, HxDev);
    st = WdfDeviceCreate(&init, &attr, &device);
    if (!NT_SUCCESS(st)) return st;

    HxDev *d = HxGetDev(device);
    HxZero(d, sizeof(*d));
    d->wdf = device;
    InitializeSRWLock(&d->lock);
    InitializeCriticalSection(&d->ctl);
    HxReadSettings(d);
    g_dev = d;

    st = WdfDeviceCreateDeviceInterface(device, &HxIddInterface, NULL);
    if (!NT_SUCCESS(st)) return st;
    return IddCxDeviceInitialize(device);
}

extern "C" NTSTATUS DriverEntry(PDRIVER_OBJECT obj, PUNICODE_STRING path)
{
    WDF_DRIVER_CONFIG cfg;
    WDF_DRIVER_CONFIG_INIT(&cfg, HxDeviceAdd);
    return WdfDriverCreate(obj, path, WDF_NO_OBJECT_ATTRIBUTES, &cfg, WDF_NO_HANDLE);
}
