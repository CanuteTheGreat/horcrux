vmtest: see docs/design/vmtest.md.  Quick start:
    python3 -m vmtest run scenarios/selftest_alpine.py --out /tmp/vmt     # 3 OpenRC guests, ~1-2 min with KVM
    python3 -m unittest discover -s tests                                 # no QEMU needed
