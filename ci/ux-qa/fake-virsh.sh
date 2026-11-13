#!/bin/sh
# Stand-in for `virsh` (HORCRUX_VIRSH=this file): two libvirt domains that Horcrux does not manage.  Used by check_foreign_vms.py.
shift 2 2>/dev/null   # drop: -c <uri>
case "$1 $2 $3" in
  "list --all --name") printf 'uxqa-foreign-1\nuxqa-foreign-2\n\n' ;;
  domstate*) echo "shut off" ;;
  dumpxml*)
    n="$3"; [ "$1 $2" = "dumpxml --inactive" ] || n="$2"
    cat <<XML
<domain type='kvm'><name>$n</name><uuid>$(printf '%s' "$n" | md5sum | sed 's/\(........\)\(....\)\(....\)\(....\)\(............\).*/\1-\2-\3-\4-\5/')</uuid>
<memory unit='KiB'>1048576</memory><vcpu>2</vcpu><os><type arch='x86_64' machine='pc-q35-8.0'>hvm</type></os>
<devices><disk type='file' device='disk'><driver name='qemu' type='qcow2'/><source file='/var/lib/libvirt/images/$n.qcow2'/><target dev='vda' bus='virtio'/></disk>
<interface type='network'><mac address='52:54:00:aa:bb:01'/><source network='default'/><model type='virtio'/></interface></devices></domain>
XML
    ;;
  *) exit 0 ;;
esac
