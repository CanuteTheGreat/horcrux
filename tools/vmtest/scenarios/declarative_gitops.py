"""Declarative GitOps on two installed Horcrux nodes that follow one git repository.

n1 follows the repo in `auto` mode, n2 in `approve` mode; both verify commit signatures against one SSH signer.
The harness serves the repository with `git daemon` (the guests reach the host at 10.0.2.2 through QEMU user networking).

Proven, on real guests and the guests' own init system (OpenRC or systemd: this script never names an init):
 - a signed commit converges both nodes (n1 applies it, n2 queues a plan and applies it on approval)
 - a hand change on a node (drift) is visible through the drift API with expected/actual per field,
   corrected on the next sync in auto mode, and shown as a pending plan in approve mode (nothing changes until approved)
 - an unsigned commit is refused on both nodes with the reason, and neither node changes
 - the scheduled sync (no manual trigger) follows a new signed commit
 - the apply history records the gitops applies with their commit

    python3 -m vmtest run scenarios/declarative_gitops.py --arg iso=/path/horcrux-server-openrc-daily-x86_64.iso
    # development only: --arg api_bin=/path/horcrux-api replaces the guests' horcrux-api binary (never used by CI)
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from vmtest import Disk, Nic, NodeSpec, Test  # noqa: E402
from vmtest.recipes import PROJECTS, PW, make_answers, unattended_install  # noqa: E402

P = PROJECTS["horcrux"]
HOST = "10.0.2.2"          # the host, as seen from a QEMU user-mode guest
VM = "fleet-vm"


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class Repo:
    """A bare repository served by git daemon, plus a work tree on the host to make (signed) commits in."""

    def __init__(self, root):
        self.root = root
        self.work = os.path.join(root, "work")
        self.srv = os.path.join(root, "srv")
        os.makedirs(self.srv)
        self.bare = os.path.join(self.srv, "fleet.git")
        self.git("init", "-q", "-b", "main", "--bare", self.bare, cwd=root)
        self.git("init", "-q", "-b", "main", self.work, cwd=root)
        self.port = free_port()
        self.daemon = subprocess.Popen(
            ["git", "daemon", "--reuseaddr", f"--base-path={self.srv}", "--export-all", "--listen=0.0.0.0", f"--port={self.port}"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.key = os.path.join(root, "signer")
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "fleet-ci", "-f", self.key])
        self.rogue = os.path.join(root, "rogue")
        subprocess.check_call(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "rogue", "-f", self.rogue])
        self.allowed = "ci@example.test " + open(self.key + ".pub").read().strip()

    @staticmethod
    def git(*args, cwd):
        env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1")
        return subprocess.check_output(["git", "-c", "user.name=CI", "-c", "user.email=ci@example.test", *args], cwd=cwd, env=env).decode().strip()

    @property
    def url(self):
        return f"git://{HOST}:{self.port}/fleet.git"

    def commit(self, mem, signed=True, key=None, msg=None):
        spec = f"version: 1\nvms:\n  - {{name: {VM}, memory: {mem}, cpus: 1, power: stopped}}\n"
        with open(os.path.join(self.work, "fleet.yaml"), "w") as f:
            f.write(spec)
        self.git("add", "-A", cwd=self.work)
        sign = ["-c", "gpg.format=ssh", "-c", f"user.signingkey={key or self.key}", "commit", "-S"] if signed else ["commit"]
        self.git(*sign, "-q", "-m", msg or f"{VM} memory {mem}", cwd=self.work)
        sha = self.git("rev-parse", "HEAD", cwd=self.work)
        self.git("push", "-q", self.bare, "HEAD:refs/heads/main", cwd=self.work)
        return sha

    def stop(self):
        self.daemon.terminate()
        try:
            self.daemon.wait(10)
        except subprocess.TimeoutExpired:
            self.daemon.kill()


def define(args):
    nodes = []
    keys = []
    for name in ("n1", "n2"):
        answers, key = make_answers("horcrux")
        keys.append(key)
        nodes.append(NodeSpec(name=name, mem=int(args.get("mem", 3072)), cpus=2, iso=args["iso"], iso_media="usb", firmware="uefi",
                              disks=[Disk("24G")], extra_drives=[(answers, "usb")], nics=[Nic("user", forwards={"ssh": 22, "web": P["port"]})],
                              ssh_key=key, console="none", boot_timeout=420))

    def run(m):
        root = tempfile.mkdtemp(prefix="vmt-gitops-")
        repo = Repo(root)
        try:
            body(m, repo)
        finally:
            repo.stop()
            shutil.rmtree(root, ignore_errors=True)
            for k in keys:
                shutil.rmtree(os.path.dirname(k), ignore_errors=True)

    def body(m, repo):
        n1, n2 = m.n1, m.n2
        tok = {}

        def api(n, method, path, payload=None):
            hdr = {"Authorization": f"Bearer {tok[n.name]}"} if n.name in tok else None
            sc, data = n.http("web", "/api" + path, method, payload, headers=hdr, timeout=60)
            try:
                return sc, json.loads(data)
            except ValueError:
                return sc, data.decode(errors="replace")

        def vm_memory(n):
            sc, vms = api(n, "GET", "/vms")
            return next((v["memory"] for v in vms if v["name"] == VM), None) if sc == 200 else None

        m.phase("install both nodes from the ISO and boot them")
        for n in (n1, n2):
            unattended_install(m, n)
        for n in (n1, n2):
            n.start()
        for n in (n1, n2):
            m.check(f"{n.name}:installed-boots", n.q.expect(r"login:", n.T(420)), "")
            n.wait_for_ssh(120)
            n.wait_for_unit(P["svc"], 300)
            m.check(f"{n.name}:{P['svc']}-active", True, n.init.name)
            n.wait_until(lambda n=n: n.http("web", P["health"])[0] == 200, 300, 5, "web health 200")
            sc, b = n.http("web", "/api/auth/login", "POST", {"username": P["web_user"], "password": PW})
            data = json.loads(b) if sc == 200 else {}
            tok[n.name] = data.get("ticket") or data.get("token")
            m.check(f"{n.name}:admin-login", bool(tok[n.name]), f"{sc}")
            if args.get("api_bin"):
                # development only: put a locally built API binary on the guest (CI always tests the ISO's own)
                path = n.succeed("command -v horcrux-api").strip()
                n.put_file(args["api_bin"], "/tmp/horcrux-api.new", "755")
                n.succeed(f"cp /tmp/horcrux-api.new {path}")
                n.restart_unit(P["svc"])
                n.wait_for_unit(P["svc"], 120)
                n.wait_until(lambda n=n: n.http("web", P["health"])[0] == 200, 300, 5, "web health after the binary override")
                sc, b = n.http("web", "/api/auth/login", "POST", {"username": P["web_user"], "password": PW})
                tok[n.name] = json.loads(b).get("ticket")
                m.note(f"{n.name}:DEV-OVERRIDE", True, "horcrux-api replaced by --arg api_bin (not an ISO test)")
            sc, st = api(n, "GET", "/declarative/gitops/status")
            m.check(f"{n.name}:iso-has-declarative-gitops", sc == 200, f"GET /api/declarative/gitops/status -> {sc} "
                    + ("" if sc == 200 else "(this ISO predates declarative GitOps; use a daily built after the feature merged)"))

        m.phase("configure GitOps: n1 auto, n2 approve, both verify signatures")
        for n, mode in ((n1, "auto"), (n2, "approve")):
            sc, v = api(n, "PUT", "/declarative/gitops/config", {
                "url": repo.url, "branch": "main", "path": "fleet.yaml", "mode": mode, "interval_secs": 3600,
                "verify_signatures": True, "signing": {"format": "ssh", "allowed_signers": repo.allowed},
                "node_name": n.name})
            m.check(f"{n.name}:gitops-config-saved", sc == 200, f"{sc} {v if sc != 200 else mode}")

        m.phase("a signed commit converges both nodes")
        c1 = repo.commit(128)
        sc, o = api(n1, "POST", "/declarative/gitops/sync")
        m.check("n1:auto-applied", sc == 200 and o.get("applied") is True, f"{sc} {o}")
        sc, o = api(n2, "POST", "/declarative/gitops/sync")
        pending = o.get("pending") if sc == 200 and isinstance(o, dict) else None
        m.check("n2:queued-for-approval", bool(pending) and vm_memory(n2) is None, f"{sc} {o}; nothing applied before approval")
        sc, rep = api(n2, "POST", f"/declarative/gitops/approve/{pending}")
        m.check("n2:approved-and-applied", sc == 200 and rep.get("ok") is True, f"{sc}")
        m.check("both-nodes-converged", vm_memory(n1) == 128 and vm_memory(n2) == 128, f"n1={vm_memory(n1)} n2={vm_memory(n2)}")
        for n in (n1, n2):
            sc, st = api(n, "GET", "/declarative/gitops/status")
            m.check(f"{n.name}:status-commit-and-signature", st.get("last_commit") == c1 and st.get("signature") == "good", f"{st.get('last_commit')} {st.get('signature')}")

        m.phase("drift by hand: visible, corrected (auto) or pending (approve)")
        for n in (n1, n2):
            sc, v = api(n, "PUT", f"/vms/{VM}/config", {"memory": 300})
            m.check(f"{n.name}:hand-change-applied", sc == 200, f"{sc}")
            sc, d = api(n, "POST", "/declarative/drift/check")
            item = next((i for i in d.get("items", []) if i["name"] == VM and i["field"] == "memory"), None)
            m.check(f"{n.name}:drift-detected", bool(item) and item["expected"] == "128" and item["actual"] == "300", str(item))
        sc, o = api(n1, "POST", "/declarative/gitops/sync")
        m.check("n1:drift-corrected-on-sync", sc == 200 and vm_memory(n1) == 128, f"{sc} memory={vm_memory(n1)}")
        sc, o = api(n2, "POST", "/declarative/gitops/sync")
        pend = o.get("pending") if sc == 200 and isinstance(o, dict) else None
        m.check("n2:drift-shown-pending-not-corrected", bool(pend) and vm_memory(n2) == 300, f"pending={pend} memory={vm_memory(n2)}")
        sc, st = api(n2, "GET", "/declarative/gitops/status")
        plan = (st.get("pending") or [{}])[0].get("plan", {}).get("changes", [])
        m.check("n2:pending-plan-is-an-update", [c["action"] for c in plan] == ["update"], str(plan))
        sc, rep = api(n2, "POST", f"/declarative/gitops/approve/{pend}")
        m.check("n2:approve-corrects-drift", sc == 200 and vm_memory(n2) == 128, f"{sc} memory={vm_memory(n2)}")

        m.phase("an unsigned commit is refused")
        bad = repo.commit(256, signed=False, msg="unsigned change")
        for n in (n1, n2):
            sc, o = api(n, "POST", "/declarative/gitops/sync")
            m.check(f"{n.name}:unsigned-refused", sc == 422 and "signature verification failed" in json.dumps(o), f"{sc} {str(o)[:200]}")
            sc, st = api(n, "GET", "/declarative/gitops/status")
            m.check(f"{n.name}:refusal-reported", st.get("signature") == "bad" and st.get("last_commit") == bad
                    and "signature verification failed" in (st.get("last_error") or ""), str(st.get("last_error"))[:160])
            m.check(f"{n.name}:unchanged-after-refusal", vm_memory(n) == 128, f"memory={vm_memory(n)}")
        repo.commit(384, signed=True, key=repo.rogue, msg="signed by a key nobody allowed")
        sc, o = api(n1, "POST", "/declarative/gitops/sync")
        m.check("n1:unknown-signer-refused", sc == 422 and vm_memory(n1) == 128, f"{sc}")

        m.phase("a new signed commit; the scheduled sync follows it without a manual trigger")
        c3 = repo.commit(192, msg="back to signed: 192")
        sc, o = api(n1, "PUT", "/declarative/gitops/config", {
            "url": repo.url, "branch": "main", "path": "fleet.yaml", "mode": "auto", "interval_secs": 10,
            "verify_signatures": True, "signing": {"format": "ssh", "allowed_signers": repo.allowed}, "node_name": "n1"})
        m.check("n1:interval-10s", sc == 200, f"{sc}")
        n1.wait_until(lambda: vm_memory(n1) == 192, n1.T(150), 5, "n1 follows the commit on its own schedule")
        m.check("n1:scheduled-sync-applied", True, "no manual sync was called")
        sc, o = api(n2, "POST", "/declarative/gitops/sync")
        pend = o.get("pending") if sc == 200 and isinstance(o, dict) else None
        sc, rep = api(n2, "POST", f"/declarative/gitops/approve/{pend}")
        m.check("n2:approved-192", sc == 200 and vm_memory(n2) == 192, f"{sc} memory={vm_memory(n2)}")

        m.phase("history")
        for n in (n1, n2):
            sc, h = api(n, "GET", "/declarative/history")
            srcs = [i["source"] for i in h.get("items", [])]
            m.check(f"{n.name}:history-records-gitops-applies", sc == 200 and srcs.count("gitops") >= 2 and any(i.get("commit") for i in h["items"]), str(srcs))
        sc, h = api(n1, "GET", "/declarative/history")
        newest = h["items"][0]["id"]
        sc, pv = api(n1, "POST", f"/declarative/history/{newest}/rollback", {"dry_run": True})
        m.check("n1:rollback-preview", sc == 200 and pv.get("dry_run") is True and vm_memory(n1) == 192, f"{sc}")
        m.note("commits", True, f"c1={c1[:12]} unsigned={bad[:12]} c3={c3[:12]}")

        for n in (n1, n2):
            n.power_off()

    return Test("declarative-gitops", nodes=nodes, networks=[], run=run, timeout=int(args.get("timeout", 7200)),
                description="two installed Horcrux nodes follow one git repo: converge, drift, approval, signature refusal, scheduled sync")
