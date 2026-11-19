FROM archlinux:latest
RUN pacman -Syu --noconfirm --needed \
      qemu-base qemu-hw-display-virtio-gpu qemu-hw-display-virtio-gpu-gl \
      qemu-hw-display-virtio-vga qemu-hw-display-virtio-vga-gl \
      qemu-hw-display-virtio-gpu-pci qemu-hw-display-virtio-gpu-pci-gl \
      qemu-ui-egl-headless qemu-ui-opengl virglrenderer \
      mesa vulkan-intel vulkan-swrast vulkan-tools vkmark \
      libisoburn openssh python socat procps-ng \
 && pacman -Scc --noconfirm
