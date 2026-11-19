#!/bin/bash
# trace-bw.sh (container): measure the guest->host Venus/virgl command stream volume (virtio-gpu ctx submits)
# during the vkQuake timedemo, as the lower bound of what a remote-GPU transport must carry.
. /w/lib.sh
Q() { python3 /w/qmp.py /w/qmp.sock "$@"; }
Q '{"execute":"quit"}' >/dev/null 2>&1; sleep 2
rm -f /w/trace.log
/w/run-vm.sh venus -D /w/trace.log >/dev/null
waitssh 120 || exit 1
Q '{"execute":"trace-event-set-state","arguments":{"name":"virtio_gpu_cmd_ctx_submit","enable":true}}' >/dev/null
Q '{"execute":"trace-event-set-state","arguments":{"name":"virtio_gpu_cmd_res_create_blob","enable":true}}' >/dev/null
$G "bash /root/quake.sh 1" | grep run
Q '{"execute":"trace-event-set-state","arguments":{"name":"virtio_gpu_cmd_ctx_submit","enable":false}}' >/dev/null
python3 - <<'EOF'
import re, collections
sub=collections.Counter(); cnt=collections.Counter(); blobs=0; blobsz=0
for l in open('/w/trace.log', errors='ignore'):
    m=re.search(r'virtio_gpu_cmd_ctx_submit.*ctx 0x([0-9a-f]+), size (\d+)', l)
    if m: sub[m.group(1)]+=int(m.group(2)); cnt[m.group(1)]+=1; continue
    m=re.search(r'virtio_gpu_cmd_res_create_blob.*size (\d+)', l)
    if m: blobs+=1; blobsz+=int(m.group(1))
for c in sub: print(f"ctx {c}: {cnt[c]} submits, {sub[c]/1e6:.2f} MB")
print(f"total {sum(sub.values())/1e6:.2f} MB in {sum(cnt.values())} submits; blobs created {blobs} ({blobsz/1e6:.1f} MB)")
EOF
grep -m2 virtio_gpu_cmd_ctx_submit /w/trace.log
