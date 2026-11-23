#!/bin/sh
# normalize-portage-perms.sh <rootfs-dir>
# Image build gate: everything under <root>/etc/portage (except gnupg/, which holds keys) is readable and traversable by every user:
# directories 0755, files 0644 (0755 when already executable), never group/world-writable. Images are assembled from layers COPYed out
# of git checkouts and written by RUN steps, so a build host or a layer author with umask 077 (checkout files arrive 0600, dirs 0700)
# would otherwise ship a /etc/portage that an administrator cannot read: `emerge -p`, `equery` and `eix` run as the user (portage group)
# and Portage's unprivileged build user reads repos.conf and package.*. Symlinks are left alone (chmod would follow them out of the tree).
set -eu
root=${1:?usage: normalize-portage-perms.sh <rootfs-dir>}
d=$root/etc/portage
[ -d "$d" ] || { echo "normalize-portage-perms: no $d, nothing to do"; exit 0; }
# the directories above it must be traversable too
for a in "$root/etc" "$d"; do
	chmod go+rx "$a"
done
bad=$(find "$d" -path "$d/gnupg" -prune -o ! -type l \( \( -type d ! -perm -055 \) -o \( -type f ! -perm -044 \) -o -perm /022 \) -print | wc -l)
find "$d" -path "$d/gnupg" -prune -o ! -type l \( -type d -o -type f \) -exec chmod u+rwX,go+rX,go-w {} +
echo "normalize-portage-perms: fixed $bad entr$([ "$bad" = 1 ] && echo y || echo ies) under /etc/portage"
