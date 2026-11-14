#!/bin/bash
# Horcrux/Patronus systemd images build the SAME script-based initramfs as the OpenRC images (every systemd* dracut module is omitted,
# /etc/dracut.conf.d/05-init.conf). Some file still pulls /usr/bin/systemctl into the image, and upstream scripts take its mere presence as
# "this initrd runs systemd": OpenZFS's zfs-load-key.sh then waits forever for `systemctl is-active zfs-import.target` and a root-on-ZFS
# system never boots. This module (only when the systemd dracut module is NOT included) drops systemctl from the image and, as a guard,
# again at the very start of the boot, before any hook looks for it.
check() {
	return 0
}

depends() {
	return 0
}

install() {
	dracut_module_included "systemd" && return 0
	rm -f "${initdir}/usr/bin/systemctl" "${initdir}/bin/systemctl"
	inst_hook cmdline 00 "${moddir}/scripted-init.sh"
}
