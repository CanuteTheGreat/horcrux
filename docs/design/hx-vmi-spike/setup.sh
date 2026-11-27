set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq qemu-system-x86 qemu-utils openssh-client python3 curl ca-certificates git cmake build-essential pkg-config libglib2.0-dev libjson-c-dev libyajl-dev flex bison libtool autoconf automake sysbench >/dev/null 2>&1 || apt-get install -y qemu-system-x86 qemu-utils openssh-client python3 curl ca-certificates git cmake build-essential pkg-config libglib2.0-dev libjson-c-dev libyajl-dev flex bison libtool autoconf automake | tail -3
cd /work
[ -f base.qcow2 ] || { curl -sfLO https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2 && mv debian-13-genericcloud-amd64.qcow2 base.qcow2; }
ls -la
