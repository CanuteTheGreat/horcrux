"""Machines: the object a test script receives (`m`).  Owns nodes, networks, steps, artifacts."""
from __future__ import annotations

import json
import os
import shutil
import time
import traceback

from .netsim import Hub
from .node import Node, VmtestError
from .qemu import kill_all, kvm_available
from .spec import Test


class StepFailed(VmtestError):
    pass


def cgroup_mem_limit_mb():
    for p in ("/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory/memory.limit_in_bytes"):
        try:
            v = open(p).read().strip()
            if v.isdigit() and int(v) < 1 << 50:
                return int(v) // (1 << 20)
        except OSError:
            pass
    return None


class Machines:
    def __init__(self, test: Test, out: str, args: dict | None = None, slow: float = 0):
        self.test, self.out, self.args = test, os.path.abspath(out), args or {}
        os.makedirs(self.out, exist_ok=True)
        self.kvm = all(kvm_available(n.arch) for n in test.nodes)
        self.slow = slow or float(os.environ.get("VMTEST_SLOW", 0) or 0) or (1.0 if self.kvm else 4.0)   # TCG is ~4x slower: scale every wait
        self.steps: list[dict] = []
        self.t0 = time.time()
        self.nets = {}
        for n in test.networks:
            if n.kind == "hub":
                self.nets[n.name] = Hub(n.name, self.out, n.pcap)
            else:
                self.nets[n.name] = n.bridge
        self.nodes: dict[str, Node] = {}
        for i, spec in enumerate(test.nodes, 1):
            self.nodes[spec.name] = Node(spec, i, self)
        self._check_memory()

    def _check_memory(self):
        lim = cgroup_mem_limit_mb()
        want = sum(n.spec.mem for n in self.nodes.values())
        if lim and want + 512 > lim:
            raise VmtestError(f"guests want {want} MB but this container's memory cap is {lim} MB: a guest as large as the cap is OOM-killed "
                              f"once it has touched its RAM. Lower Node(mem=...) or raise the job's memory.")

    # ---- access ----------------------------------------------------------------------
    def __getitem__(self, name):
        return self.nodes[name]

    def __getattr__(self, name):
        try:
            return self.__dict__["nodes"][name]
        except KeyError:
            raise AttributeError(name)

    def network(self, name):
        return self.nets[name]

    def start_all(self):
        for n in self.nodes.values():
            n.start()
        return self

    def wait_all_shells(self, timeout=None):
        for n in self.nodes.values():
            n.wait_for_shell(timeout)

    def snapshot_all(self, tag):
        """Consistent multi-node checkpoint: freeze all, savevm each, resume (qcow2 disks only)."""
        for n in self.nodes.values():
            n.freeze()
        try:
            for n in self.nodes.values():
                n.snapshot(tag)
        finally:
            for n in self.nodes.values():
                n.thaw()

    def restore_all(self, tag):
        for n in self.nodes.values():
            n.restore(tag)
            n.shell_ready = True

    # ---- steps / results -----------------------------------------------------------------
    def step(self, name, ok, detail="", advisory=False):
        self.steps.append({"step": name, "ok": bool(ok), "advisory": advisory, "detail": str(detail)[:400], "t": round(time.time() - self.t0, 1)})
        print(f"[{'PASS' if ok else 'WARN' if advisory else 'FAIL'}] {name}  {str(detail)[:200]}", flush=True)
        return bool(ok)

    def note(self, name, ok, detail=""):
        """Advisory result: reported, never fails the run."""
        return self.step(name, ok, detail, advisory=True)

    def check(self, name, ok, detail=""):
        """Record a step; a false one aborts the test (evidence is collected by the runner)."""
        if not self.step(name, ok, detail):
            raise StepFailed(f"{name}: {detail}")

    def phase(self, title):
        print(f"\n== {title}", flush=True)

    # ---- artifacts / teardown ----------------------------------------------------------------
    def collect(self):
        for n in self.nodes.values():
            n.collect()
        for name, h in self.nets.items():
            if isinstance(h, Hub):
                open(os.path.join(self.out, f"net-{name}.events.txt"), "w").write(
                    "\n".join(h.events) + f"\nstats: {h.stats}\n")

    def teardown(self, keep_disks=False):
        for n in self.nodes.values():
            if n.q:
                n.q.kill()
        kill_all()
        for h in self.nets.values():
            if isinstance(h, Hub):
                h.close()
        if not keep_disks:   # 20-40 GB of regenerable state: only logs, PNGs, pcaps and result.json stay
            for n in self.nodes.values():
                for f in os.listdir(n.out):
                    if f.endswith((".qcow2", ".raw", ".img")) or f.endswith(".qmp") or f.endswith(".serial"):
                        try:
                            os.unlink(os.path.join(n.out, f))
                        except OSError:
                            pass

    def result(self, ok, error=""):
        r = {"test": self.test.name, "ok": ok, "error": error, "accel": "kvm" if self.kvm else "tcg", "slow": self.slow,
             "seconds": round(time.time() - self.t0, 1), "steps": self.steps}
        json.dump(r, open(os.path.join(self.out, "result.json"), "w"), indent=1)
        return r
