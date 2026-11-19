#!/bin/bash
# dump.sh (container): replay the trimmed capture on ANV and lavapipe, dumping the render target of the first
# replayed draw, so the reconstructed state can be compared pixel by pixel.
cd /w/cap
F=${1:-venus_queue_submits_300_through_302.gfxr}
cat > dump.json <<'EOF'
{"BeginCommandBuffer":[77],"Draw":[[82]],"RenderPass":[[[78,83]]],"QueueSubmit":[114]}
EOF
H=$(/w/gfxr/build/tools/replay/gfxrecon-replay --help 2>&1)
fmt=""; echo "$H" | grep -q -- '--dump-resources-image-format' && fmt="--dump-resources-image-format png"
for icd in intel_icd lvp_icd; do
  rm -rf dump-$icd; mkdir dump-$icd
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/$icd.json /w/gfxr/build/tools/replay/gfxrecon-replay --remove-unsupported -m rebind \
     --dump-resources dump.json --dump-resources-dir dump-$icd $fmt $F > dump-$icd.log 2>&1
  echo "$icd rc=$?"; ls dump-$icd | head -8
  grep -iE 'error|fatal' dump-$icd.log | head -3
done
