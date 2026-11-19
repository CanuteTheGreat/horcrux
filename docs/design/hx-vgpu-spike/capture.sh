#!/bin/bash
# capture.sh (container): start the Venus guest with the GFXReconstruct capture layer inside the HOST render server,
# run a Vulkan workload in the guest, trim-capture its live state at queue submit N, then replay that state
# on the host's Intel GPU (ANV) and on a different "vendor" (lavapipe, CPU).
. /w/lib.sh
Q() { python3 /w/qmp.py /w/qmp.sock "$@"; }
R=${R:-300-302}
mkdir -p /w/cap; rm -f /w/cap/*
Q '{"execute":"quit"}' >/dev/null 2>&1; sleep 2
export VK_LAYER_PATH=/w/gfxr/build/layer VK_INSTANCE_LAYERS=VK_LAYER_LUNARG_gfxreconstruct \
  GFXRECON_CAPTURE_FILE=/w/cap/venus.gfxr GFXRECON_CAPTURE_FILE_TIMESTAMP=false \
  GFXRECON_CAPTURE_QUEUE_SUBMITS=$R GFXRECON_MEMORY_TRACKING_MODE=${MT:-unassisted} \
  GFXRECON_LOG_FILE=/w/cap/gfxr.log GFXRECON_LOG_LEVEL=info
/w/run-vm.sh venus >/dev/null
unset VK_LAYER_PATH VK_INSTANCE_LAYERS
waitssh 120 || exit 1
$G "vkmark --winsys headless -s 1280x720 -b ${SCENE:-shading:shading=phong:duration=4}" | grep -E 'FPS|Device Name'
sleep 2; ls -l /w/cap/; tail -5 /w/cap/gfxr.log
for f in /w/cap/venus*.gfxr; do
  /w/gfxr/build/tools/info/gfxrecon-info $f 2>&1 | grep -iE "Total frames|API calls|Driver|Device name|Application|Vulkan" | head -8
  for icd in intel_icd lvp_icd; do
    t0=$(date +%s.%N)
    VK_DRIVER_FILES=/usr/share/vulkan/icd.d/$icd.json timeout 120 /w/gfxr/build/tools/replay/gfxrecon-replay ${RFLAGS:-} $f > /w/cap/replay-$icd.log 2>&1
    rc=$?; t1=$(date +%s.%N)
    echo "replay on $icd: rc=$rc $(echo "$t1-$t0" | bc) s; $(grep -ciE 'error|fatal' /w/cap/replay-$icd.log) error lines"
    grep -iE 'error|fatal|warn' /w/cap/replay-$icd.log | sort | uniq -c | sort -rn | head -4
  done
done
