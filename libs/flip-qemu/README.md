# Flip's QEMU

QEMU with virglrenderer built with Venus, for My Flip's one VM on Linux computers whose GPU has
no gVisor proxy (AMD, Intel, any other). The VM is then a KVM guest that reaches the GPU through
virtio-gpu: OpenGL through virgl and Vulkan through Venus. The guest's GL and Vulkan streams are
decoded on the host by virglrenderer inside QEMU's process, which runs as Flip's service user.

`build.sh <work dir> <out dir>` builds `flip-qemu-<label>-<arch>.tar.gz` for the machine it runs
on, from the versions in `versions.env`: QEMU's release tarball (checked by SHA-256),
virglrenderer and Vulkan-Headers (checked by the commit their tag names). The tarball has
`bin/` (the system emulator and qemu-img), `lib/` (the libraries the binaries need that a
computer may lack, found through `$ORIGIN/../lib`), `share/qemu` and `VERSIONS` (versions, venus,
and the oldest glibc the files need). The host GPU's own user-space (libgbm, libEGL, libGL,
libvulkan, libdrm) is not shipped: it belongs to the computer's driver.

The build fails unless the emulator has `virtio-gpu-gl-pci` with its `venus` property. The
`qemu-linux` job of `cd-cua-spacesd.yml` runs it for each release (x86_64; no ARM Linux runner
exists yet, so aarch64 computers get no QEMU build and keep the gVisor VM without a GPU).
