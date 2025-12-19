# Horcrux Stage3 Specification
# This builds a minimal stage3 tarball optimized for virtualization

subarch: amd64
target: stage3
version_stamp: horcrux-latest
rel_type: horcrux
profile: default/linux/amd64/23.0
snapshot: latest
source_subpath: horcrux/stage3-amd64-openrc-latest

# Use generic x86-64 for maximum compatibility
cflags: -O2 -pipe -march=x86-64 -mtune=generic
cxxflags: -O2 -pipe -march=x86-64 -mtune=generic

# Portage configuration
portage_confdir: /home/canutethegreat/files/repos/mine/horcrux/build/catalyst/files/portage

# Common options for all stages
common_flags: -O2 -pipe -march=x86-64 -mtune=generic
