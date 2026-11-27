# Guest introspection spike (security suite phase 4)

Throwaway Debian 13 cloud guest (kernel 6.12, 2 vCPU, 2 GiB) under QEMU 10 / KVM, boot.sh. The guest RAM is a shared memory-backend-file;
vmi.py/scan.py (Python, stdlib only) read it read-only, walk the guest page tables (CR3 and IDT from QMP `info registers`),
derive the KASLR slide from IDT gate 14, then walk init_task->tasks and the syscall table. pause.py and meas.py take the
timings. res_*.txt are the raw benchmark outputs (run on a shared, loaded host: load average about 20 on 8 cores).
Findings and the go/no-go: docs/design/security-suite.md, "Phase 4 spike".
