#!/bin/bash
# bench.sh WHERE LABEL  (WHERE=host|guest) -> /w/results/LABEL-vkmark.txt
. /w/lib.sh
mkdir -p /w/results
W=$1; L=$2; shift 2
cmd="vkmark --winsys headless -s 1920x1080 $*"
if [ "$W" = host ]; then
  VK_DRIVER_FILES=${VK_DRIVER_FILES:-/usr/share/vulkan/icd.d/intel_icd.json} $cmd > /w/results/$L-vkmark.txt 2>&1
else
  $G "$cmd" > /w/results/$L-vkmark.txt 2>&1
fi
grep -E 'fps|Score|GPU|Device' /w/results/$L-vkmark.txt | sed 's/^ *//' | tr '\n' ';'; echo
