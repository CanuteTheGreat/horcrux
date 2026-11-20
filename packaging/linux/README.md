# Linux guest tools packaging

`nfpm.yaml` + `prerm.sh` build `horcrux-guest-tools` as deb, rpm and apk
(`ci/build-guest-tools-pkgs.sh`), laid out like the Gentoo ebuild: agent,
`/etc/horcrux/guest-tools.conf` (kept on upgrade), systemd unit and OpenRC
script. Not enabled on install; removal stops/disables under either init.
Dependency: `ffmpeg` for deb and apk; for rpm the file `/usr/bin/ffmpeg`
(Fedora's `ffmpeg-free` provides no `ffmpeg` capability, but owns the file;
RPM Fusion/openSUSE `ffmpeg` too). ffmpeg-free has no libx264, so software
encode on Fedora needs RPM Fusion's ffmpeg. Override with `FFMPEG_DEP`. CI install-tests each
package in debian:trixie, fedora:latest and alpine:latest.
