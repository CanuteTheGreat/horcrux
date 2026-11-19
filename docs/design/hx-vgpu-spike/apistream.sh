#!/bin/bash
# apistream.sh (container): full host-side Vulkan API capture (GFXReconstruct layer in the render server) of the
# guest's vkQuake timedemo = the volume an API-level remote-GPU transport would carry (unassisted memory mode:
# every mapped allocation that may have changed is shipped at each submit, so this is an upper bound).
. /w/lib.sh
Q() { python3 /w/qmp.py /w/qmp.sock "$@"; }
MT=${MT:-unassisted}
mkdir -p /w/cap2; rm -f /w/cap2/*
Q '{"execute":"quit"}' >/dev/null 2>&1; sleep 2
export VK_LAYER_PATH=/w/gfxr/build/layer VK_INSTANCE_LAYERS=VK_LAYER_LUNARG_gfxreconstruct \
  GFXRECON_CAPTURE_FILE=/w/cap2/q.gfxr GFXRECON_CAPTURE_FILE_TIMESTAMP=true \
  GFXRECON_MEMORY_TRACKING_MODE=$MT GFXRECON_LOG_FILE=/w/cap2/gfxr.log ${R:+GFXRECON_CAPTURE_QUEUE_SUBMITS=$R}
/w/run-vm.sh venus >/dev/null
unset VK_LAYER_PATH VK_INSTANCE_LAYERS
waitssh 120 || exit 1
$G "bash /root/quake.sh 1" | grep run
sleep 3
ls -l /w/cap2/*.gfxr
for f in /w/cap2/*.gfxr; do /w/gfxr/build/tools/info/gfxrecon-info $f 2>/dev/null | grep -E "Application name|Total allocations|Compression"; done
