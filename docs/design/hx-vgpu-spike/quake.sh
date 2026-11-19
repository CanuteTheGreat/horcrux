#!/bin/bash
# Run on host-container or guest: set up Quake shareware data + run vkQuake timedemo under a headless weston.
# usage: quake.sh RUNS  -> prints "timedemo: ... fps" lines
set -e
pacman -S --noconfirm --needed vkquake weston lhasa unzip >/dev/null 2>&1 || true
Q=/root/quake
if [ ! -f $Q/id1/pak0.pak ]; then
  mkdir -p /tmp/q $Q/id1; cd /tmp/q
  curl -fsSL -o quake106.zip https://ftp.gwdg.de/pub/misc/ftp.idsoftware.com/idstuff/quake/quake106.zip
  unzip -oq quake106.zip; lha -xq resource.1 >/dev/null
  cp id1/pak0.pak $Q/id1/
fi
printf "vid_vsync 0\nhost_maxfps 0\ntimedemo demo1\n" > $Q/id1/autoexec.cfg
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p $XDG_RUNTIME_DIR; chmod 700 $XDG_RUNTIME_DIR
pkill -x weston 2>/dev/null || true; sleep 1
weston --backend=headless --renderer=gl --width=${QW:-1920} --height=${QH:-1080} --refresh-rate=${REFRESH_MHZ:-1000000} --fake-seat --socket=wl-hx >/tmp/weston.log 2>&1 &
sleep 3
export WAYLAND_DISPLAY=wl-hx SDL_VIDEODRIVER=wayland
for i in $(seq 1 ${1:-3}); do
  LD_PRELOAD=/root/focus-shim.so timeout 300 stdbuf -oL vkquake -basedir $Q -width ${QW:-1920} -height ${QH:-1080} >/tmp/vkq.log 2>&1 &
  p=$!
  for t in $(seq 1 290); do grep -qE 'seconds +[0-9.]+ fps' /tmp/vkq.log 2>/dev/null && break; sleep 1; done
  kill $p 2>/dev/null; wait $p 2>/dev/null || true
  grep -E 'seconds +[0-9.]+ fps' /tmp/vkq.log | tail -1 | sed "s/^/run$i: /"
done
grep -iE 'vulkan device|GPU|Device' /tmp/vkq.log | head -3
pkill -x weston || true
