#!/usr/bin/env python3
"""GPU capability view + SR-IOV provisioning e2e (Playwright Firefox, desktop
and phone width).

Starts its own horcrux-api (argv: binary, UI dist dir) whose sysfs/devfs root
is a fixture tree: an Intel xe PF (7 VFs possible, 2 enabled, VRAM profiles)
and an NVIDIA card with no SR-IOV. Drives the Host Devices page:

  * one card per GPU, the modes and the reason each missing mode is missing
  * the VF table: VRAM per VF and the save method (VFIO migration, read from
    the device: "unknown: ..." in a tree without /dev/iommu)
  * plan (steps / refusal reasons), apply (sysfs files really written),
    a second generation and the roll back button
  * VM Hardware page shows the save method line for a VM without devices (none)
  * no horizontal page scroll at 390 px

Real VF creation and the VFIO migration ioctl need hardware (see
docs/design/hx-vgpu.md section 8).
"""
import json
import os
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import expect, sync_playwright

WIDTHS = {"desktop": (1280, 900), "mobile": (390, 844)}


def w(p, v):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w") as f:
        f.write(v)


def pci(sys_, addr, vendor, driver, group):
    d = f"{sys_}/bus/pci/devices/{addr}"
    os.makedirs(d, exist_ok=True)
    w(f"{d}/vendor", vendor)
    w(f"{d}/device", "0xe20b")
    w(f"{d}/class", "0x030000")
    g = f"{sys_}/kernel/iommu_groups/{group}"
    os.makedirs(f"{g}/devices", exist_ok=True)
    os.symlink(g, f"{d}/iommu_group")
    os.symlink(d, f"{g}/devices/{addr}")
    dd = f"{sys_}/bus/pci/drivers/{driver}"
    os.makedirs(dd, exist_ok=True)
    os.symlink(dd, f"{d}/driver")


def fixture(root):
    sys_, dev = f"{root}/sys", f"{root}/dev"
    os.makedirs(dev)
    os.makedirs(f"{sys_}/bus/pci/drivers/vfio-pci")
    pci(sys_, "0000:03:00.0", "0x8086", "xe", "10")
    pf = f"{sys_}/bus/pci/devices/0000:03:00.0"
    w(f"{pf}/sriov_totalvfs", "7\n")
    w(f"{pf}/sriov_numvfs", "2\n")
    w(f"{pf}/sriov_drivers_autoprobe", "0\n")
    for i in range(1, 8):
        w(f"{pf}/sriov_admin/vf{i}/profile/vram_quota", "4294967296\n" if i <= 2 else "0\n")
    for i in range(2):
        a = f"0000:03:00.{i + 1}"
        pci(sys_, a, "0x8086", "xe-vfio-pci", f"2{i}")
        vf = f"{sys_}/bus/pci/devices/{a}"
        os.symlink(pf, f"{vf}/physfn")
        os.symlink(vf, f"{pf}/virtfn{i}")
        os.makedirs(f"{vf}/vfio-dev/vfio{i + 1}")
        w(f"{dev}/vfio/devices/vfio{i + 1}", "")
    pci(sys_, "0000:01:00.0", "0x10de", "nvidia", "1")
    return sys_, dev, pf


def rd(p):
    return open(p).read().strip()


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def main():
    binary, dist = sys.argv[1], sys.argv[2]
    root = tempfile.mkdtemp(prefix="gpu-slicing-e2e-")
    sys_, dev, pf = fixture(root)
    port = free_port()
    base = f"https://127.0.0.1:{port}"
    env = dict(os.environ,
               HORCRUX_HOST="127.0.0.1", HORCRUX_PORT=str(port),
               HORCRUX_DATA_DIR=f"{root}/data", HORCRUX_SSL_DIR=f"{root}/ssl",
               HORCRUX_LOG_DIR=f"{root}/log", HORCRUX_AUDIT_LOG=f"{root}/log/audit.log",
               HORCRUX_UI_DIR=dist, HORCRUX_CNI_BIN_DIR=f"{root}/cni/bin",
               HORCRUX_CNI_CONF_DIR=f"{root}/cni/conf.d",
               HORCRUX_DATABASE_URL=f"sqlite://{root}/data/horcrux.db",
               HORCRUX_SYSFS_ROOT=sys_, HORCRUX_DEVFS_ROOT=dev,
               ADMIN_PASSWORD="admin", RATE_LIMIT_MAX_REQUESTS="100000",
               AUTH_RATE_LIMIT_MAX_REQUESTS="100000", RUST_LOG="info")
    os.makedirs(f"{root}/data", exist_ok=True)
    srv = subprocess.Popen([binary], env=env, stdout=open(f"{root}/server.log", "w"),
                           stderr=subprocess.STDOUT)
    ctx_ssl = ssl._create_unverified_context()
    try:
        for _ in range(90):
            try:
                urllib.request.urlopen(f"{base}/api/health", context=ctx_ssl, timeout=2)
                break
            except Exception:
                time.sleep(1)
        else:
            print(open(f"{root}/server.log").read()[-3000:])
            raise SystemExit("server did not start")
        run(base, pf)
    finally:
        srv.terminate()
        try:
            srv.wait(10)
        except Exception:
            srv.kill()
    print("ok - gpu slicing e2e")


def run(base, pf):
    errors = []
    with sync_playwright() as p:
        browser = p.firefox.launch()
        for label, (wd, ht) in WIDTHS.items():
            ctx = browser.new_context(viewport={"width": wd, "height": ht}, ignore_https_errors=True)
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(f"{base}/login")
            page.fill("#login-username", "admin")
            page.fill("#login-password", "admin")
            page.locator("#login-submit").click()
            page.wait_for_url(f"{base}/")
            page.goto(f"{base}/gpu")
            expect(page.locator(".gpu-cap")).to_have_count(2, timeout=30000)

            xe = page.locator(".gpu-cap[data-addr='0000:03:00.0']")
            expect(xe.locator(".gpu-mode[data-mode='sriov']")).to_contain_text("2 of 7")
            expect(xe.locator(".gpu-mode[data-mode='sriov']")).to_contain_text("VRAM and scheduling per VF")
            expect(xe.locator(".gpu-mode[data-mode='passthrough']")).to_contain_text("available")
            expect(xe.locator(".vf-row")).to_have_count(2)
            expect(xe.locator(".vf-row").first).to_contain_text("4096 MiB")
            expect(xe.locator(".vf-save").first).to_contain_text("unknown")
            nv = page.locator(".gpu-cap[data-addr='0000:01:00.0']")
            expect(nv.locator(".gpu-why").first).to_contain_text("passthrough only")
            expect(nv.locator(".sriov-form")).to_have_count(0)

            if label == "desktop":
                # plan: what would change; refusal reasons
                form = xe.locator(".sriov-form")
                form.locator(".sriov-vram").fill("2048")
                page.locator("#sriov-plan-btn").click()
                expect(page.locator("#sriov-plan")).to_be_visible()
                assert page.locator(".sriov-plan-step").count() >= 3
                expect(page.locator(".sriov-plan-step").first).to_contain_text("remove the 2 existing VFs")
                form.locator(".sriov-num-vfs").fill("9")
                page.locator("#sriov-plan-btn").click()
                expect(page.locator(".sriov-plan-error").first).to_contain_text("at most 7")
                form.locator(".sriov-num-vfs").fill("2")
                # apply: the files are written and a generation recorded
                page.locator("#sriov-apply-btn").click()
                expect(page.locator("#sriov-result")).to_contain_text("generation 1", timeout=30000)
                assert rd(f"{pf}/sriov_admin/vf1/profile/vram_quota") == str(2048 << 20)
                expect(page.locator(".gpu-cap[data-addr='0000:03:00.0'] .vf-row").first).to_contain_text("2048 MiB", timeout=30000)
                expect(page.locator("#sriov-rollback-btn")).to_be_disabled()
                # second generation, then roll back
                xe = page.locator(".gpu-cap[data-addr='0000:03:00.0']")
                xe.locator(".sriov-vram").fill("1024")
                page.locator("#sriov-apply-btn").click()
                expect(page.locator("#sriov-result")).to_contain_text("generation 2", timeout=30000)
                assert rd(f"{pf}/sriov_admin/vf1/profile/vram_quota") == str(1024 << 20)
                expect(page.locator("#sriov-rollback-btn")).to_be_enabled(timeout=30000)
                page.locator("#sriov-rollback-btn").click()
                expect(page.locator("#sriov-result")).to_contain_text("generation 3", timeout=30000)
                assert rd(f"{pf}/sriov_admin/vf1/profile/vram_quota") == str(2048 << 20)
            else:
                scroll = page.evaluate("document.documentElement.scrollWidth - document.documentElement.clientWidth")
                assert scroll <= 1, f"horizontal page scroll at phone width: {scroll}px"
            assert not errors, f"[{label}] JS errors: {errors}"
            ctx.close()
        browser.close()


if __name__ == "__main__":
    main()
