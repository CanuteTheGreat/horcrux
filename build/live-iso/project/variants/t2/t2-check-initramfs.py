#!/usr/bin/python3
"""build gate: the initramfs listing (lsinitrd) must contain the apple-bce stack + hid_apple, else the keyboard is dead at the prompt"""
import sys
sys.path.insert(0, "/usr/lib/live-installer")
from liveinst import t2
miss = t2.initramfs_check(open(sys.argv[1]).read())
if miss:
    sys.exit("T2 initramfs is missing: " + ", ".join(miss))
print("T2 initramfs OK: apple-bce (t2bce_core/dma/vhci) + hid_apple present")
