#!/usr/bin/env python3
"""Network notifications on an installed Horcrux GNOME VM (gnome-extra/horcrux-gnome-defaults >= 1-r14, network-notify@horcrux.errant.solutions).

Why: stock GNOME Shell posts "Connection failed / Activation of network connection failed" for EVERY NetworkManager device that enters FAILED:
a Wi-Fi roam that fails once and reconnects a second later, an access point restart, the Wi-Fi of a laptop that is online through its cable.
It names neither the network nor the interface. Horcrux replaces it with an extension (grace period, silent while another link carries the
machine, one notification that names connection + interface + reason). This test drives REAL NetworkManager failures in the VM:

  * the extension is enabled by the system defaults and ACTIVE in the logged-in session, and it replaced the stock handler (shell log);
  * a second wired link (veth pair, no DHCP server) fails its activation (FAILED, ip-config-unavailable) while the VM's own NIC is online:
    the extension logs `secondary` and posts nothing;
  * with notify-when-other-link-online=true the same failure posts ONE notification whose title names the connection and whose body names
    the interface and the reason;
  * a failure that recovers inside the grace period posts nothing (`recovered`).
Settings are put back and the test link removed afterwards.

Attach mode, same arguments as gnome-session-lifecycle-test.py (whose VM helper and greeter login it reuses):
    network-notify-vm-test.py --qmp /w/qmp.sock --port 22790 --key k [--user owner] [--out DIR]
Exit 0 = PASS, 1 = FAIL; OUT/network-notify.json holds the checks.
"""
import argparse
import importlib.util
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("gsl", os.path.join(HERE, "gnome-session-lifecycle-test.py"))
gsl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gsl)

UUID = "network-notify@horcrux.errant.solutions"
SCHEMA = "org.gnome.shell.extensions.horcrux-network-notify"
CON = "hxnn-test"


class T(gsl.Run):
    def u(self, cmd, timeout=120):
        """run as the desktop user on the session bus of the logged-in shell"""
        return self.vm.out(f"su - {self.a.user} -c 'export XDG_RUNTIME_DIR=/run/user/{self.uid} "
                           f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{self.uid}/bus; {cmd}' 2>&1", timeout=timeout)

    def shell_log(self, marker):
        # OpenRC: gnome-shell's stdout/stderr go to syslog (supervise-daemon --stdout-logger logger); systemd: the journal
        return self.vm.out(f"{{ cat /var/log/messages.0 /var/log/messages 2>/dev/null | sed -n '/{marker}/,$p'; "
                           f"journalctl -q --no-pager --since=-15min 2>/dev/null | sed -n '/{marker}/,$p'; }} | grep horcrux-network-notify")

    def fail_once(self, tag):
        """one real activation failure of the test link: no DHCP server answers on the veth peer"""
        marker = f"HXNN-{os.getpid()}-{tag}"
        self.vm.sh(f"logger -t hxnn {marker}")
        self.vm.sh(f"nmcli con up {CON} >/dev/null 2>&1; true", timeout=90)
        return marker

    def main(self):
        vm, a = self.vm, self.a
        self.uid = gsl.uid_of(vm, a.user)
        self.versions = vm.out("qlist -Iv gnome-extra/horcrux-gnome-defaults gnome-base/gnome-shell net-misc/networkmanager 2>/dev/null")
        print("versions:\n  " + self.versions.replace("\n", "\n  "), flush=True)
        if not gsl.user_shells(vm, self.uid):
            self.setup()
            if not self.login(0):
                return self.finish()
            time.sleep(20)
        info = self.wait(lambda: (lambda o: o if "ACTIVE" in o.upper().split("STATE:")[-1][:12] else None)(self.u(f"gnome-extensions info {UUID}")), 60, step=5) \
            or self.u(f"gnome-extensions info {UUID}")
        self.check(0, "extension-active-in-session", "ACTIVE" in info.upper(), " ".join(info.split())[:300])
        hooked = vm.out("cat /var/log/messages.0 /var/log/messages 2>/dev/null | grep -c 'horcrux-network-notify: stock connection-failed notification replaced'; "
                        "journalctl -q --no-pager -b 2>/dev/null | grep -c 'horcrux-network-notify: stock connection-failed notification replaced'")
        self.check(0, "stock-notification-replaced", any(x.strip().isdigit() and int(x) > 0 for x in hooked.split()), f"log lines: {hooked.split()}")

        # a second wired link that can never get an address; the VM's own NIC stays the primary connection
        vm.sh(f"nmcli con del {CON} >/dev/null 2>&1; ip link del hxnn0 2>/dev/null; ip link add hxnn0 type veth peer name hxnn1 && ip link set hxnn1 up && "
              f"sleep 2; nmcli dev set hxnn0 managed yes; sleep 2; nmcli con add type ethernet ifname hxnn0 con-name {CON} connection.autoconnect no "
              f"ipv4.method auto ipv4.dhcp-timeout 6 ipv6.method disabled >/dev/null")
        self.u(f"gsettings set {SCHEMA} failure-grace-seconds 3; gsettings set {SCHEMA} repeat-interval-seconds 0; "
               f"gsettings set {SCHEMA} notify-when-other-link-online false")
        try:
            primary = vm.out("nmcli -t -f NAME,DEVICE con show --active | grep -v -e '^lo:' -e hxnn | head -1")
            m = self.fail_once("secondary")
            log = self.wait(lambda: (lambda l: l if "hxnn0: " in l and ("secondary" in l or "notify" in l) else None)(self.shell_log(m)), 60, step=3) or self.shell_log(m)
            self.check(0, "failure-seen", "hxnn0 (" in log and "failed: ip-config-unavailable" in log, " | ".join(log.splitlines())[-400:])
            self.check(0, "silent-while-other-link-online", "hxnn0: secondary" in log and "notify:" not in log,
                       f"primary={primary!r} | " + " | ".join(log.splitlines())[-300:])

            self.u(f"gsettings set {SCHEMA} notify-when-other-link-online true")
            m = self.fail_once("notify")
            log = self.wait(lambda: (lambda l: l if "notify:" in l else None)(self.shell_log(m)), 60, step=3) or self.shell_log(m)
            line = next((x for x in log.splitlines() if "notify:" in x), "")
            self.check(0, "notification-names-connection-interface-reason",
                       f"Wired “{CON}” disconnected" in line and "hxnn0: the network gave no address (DHCP)" in line, line[-300:])

            # recovery inside the grace period: activation fails, then the link comes up with an address before the timer fires
            self.u(f"gsettings set {SCHEMA} failure-grace-seconds 25")
            m = self.fail_once("recover")
            vm.sh(f"nmcli con mod {CON} ipv4.method manual ipv4.addresses 192.0.2.10/24 && nmcli con up {CON} >/dev/null 2>&1; true", timeout=60)
            log = self.wait(lambda: (lambda l: l if "hxnn0: recovered" in l else None)(self.shell_log(m)), 60, step=3) or self.shell_log(m)
            self.check(0, "recovery-inside-grace-is-silent", "hxnn0: recovered" in log or ("failed:" in log and "notify:" not in log and "hxnn0: " not in log.split("failed:")[-1]),
                       " | ".join(log.splitlines())[-300:])
        finally:
            vm.sh(f"nmcli con del {CON} >/dev/null 2>&1; ip link del hxnn0 2>/dev/null; true")
            self.u(f"gsettings reset {SCHEMA} failure-grace-seconds; gsettings reset {SCHEMA} repeat-interval-seconds; "
                   f"gsettings reset {SCHEMA} notify-when-other-link-online")
        return self.finish()

    def finish(self):
        os.makedirs(self.a.out, exist_ok=True)
        with open(os.path.join(self.a.out, "network-notify.json"), "w") as f:
            json.dump({"result": "FAIL" if self.failed else "PASS", "versions": getattr(self, "versions", ""), "checks": self.results}, f, indent=1)
        nfail = sum(not r["ok"] for r in self.results)
        print(f"RESULT {'FAIL' if self.failed else 'PASS'}: {len(self.results) - nfail} passed, {nfail} failed", flush=True)
        return 1 if self.failed or not self.results else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--qmp", required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=22)
    ap.add_argument("--key", required=True)
    ap.add_argument("--user", default="owner")
    ap.add_argument("--out", default="out-network-notify")
    ap.add_argument("--vm-out", default=None, help="--out as seen by QEMU (screendump path) when QEMU runs in a container")
    a = ap.parse_args()
    a.cycles = 0
    sys.exit(T(a).main())


if __name__ == "__main__":
    main()
