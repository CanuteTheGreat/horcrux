#!/usr/bin/python3
"""build step: write the T2 dracut/modprobe/modules-load files + the live kernel arguments with the installer's own t2.py"""
import sys
sys.path.insert(0, "/usr/lib/live-installer")
from liveinst import t2
open("/etc/dracut.conf.d/90-horcrux-t2.conf", "w").write(t2.dracut_conf())
open("/etc/modprobe.d/90-horcrux-t2.conf", "w").write(t2.modprobe_conf())
open("/etc/modules-load.d/horcrux-t2.conf", "w").write(t2.modules_load_conf())
open("/etc/live-iso-cmdline", "w").write(t2.grub_cmdline_extra() + "\n")
print("T2 config written; live cmdline:", t2.grub_cmdline_extra())
