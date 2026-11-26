#!/usr/bin/env bash
# Builds Flip's QEMU: QEMU with virglrenderer built with Venus, so a KVM guest
# gets the host GPU's OpenGL (virgl) and Vulkan (Venus) through virtio-gpu.
# My Flip's one VM is such a guest on Linux computers whose GPU has no gVisor
# proxy (AMD, Intel).
#
#   build.sh <work dir> <out dir>
#
# Builds for the machine it runs on (x86_64 or aarch64). The work dir holds the
# sources and the build; it must be on a disk that keeps its contents (never
# /tmp). The out dir gets flip-qemu-<label>-<arch>.tar.gz with bin/ (the system
# emulator for this architecture and qemu-img), lib/ (the libraries the
# binaries need that a computer may lack), share/qemu (firmware and keymaps)
# and VERSIONS (versions, and the oldest glibc the binaries need).
#
# The host GPU's own user-space stays the computer's: libgbm, libEGL, libGL,
# libvulkan and libdrm are not shipped, because they belong to the host's
# driver. Needs: a C compiler, python3 with venv, ninja, pkg-config, git,
# patchelf and the development files of glib, pixman, libepoxy, libgbm,
# libdrm, vulkan (loader), libslirp, libseccomp, libcap-ng and zlib.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
work="${1:?work dir}"; out="${2:?out dir}"
# shellcheck source=versions.env
source "$here/versions.env"
arch="$(uname -m)"
case "$arch" in
  x86_64) target=x86_64-softmmu; emulator=qemu-system-x86_64 ;;
  aarch64) target=aarch64-softmmu; emulator=qemu-system-aarch64 ;;
  *) echo "no QEMU build for $arch" >&2; exit 2 ;;
esac
label="$QEMU_VERSION-virgl$VIRGLRENDERER_VERSION-flip.$FLIP_QEMU_BUILD"
src="$work/src"; prefix="$work/prefix"; stage="$work/stage"
mkdir -p "$src" "$out"
rm -rf "$prefix" "$stage"

# Sources, each checked: QEMU's release tarball by SHA-256, the two git
# sources by the commit their tag must name.
tarball="$src/qemu-$QEMU_VERSION.tar.xz"
[ -f "$tarball" ] || curl -fsSL -o "$tarball.part" "https://download.qemu.org/qemu-$QEMU_VERSION.tar.xz"
[ -f "$tarball.part" ] && mv "$tarball.part" "$tarball"
echo "$QEMU_SHA256  $tarball" | sha256sum -c - >/dev/null
rm -rf "$src/qemu-$QEMU_VERSION"
tar -xJf "$tarball" -C "$src"

fetch_git() { # dir url tag commit
  if [ ! -d "$1/.git" ]; then git init -q "$1"; git -C "$1" remote add origin "$2"; fi
  git -C "$1" fetch -q --depth 1 origin "refs/tags/$3:refs/tags/$3"
  got="$(git -C "$1" rev-parse "refs/tags/$3^{commit}")"
  [ "$got" = "$4" ] || { echo "$2 tag $3 names $got, want $4" >&2; exit 1; }
  git -C "$1" checkout -q --force --detach "$4"
  git -C "$1" clean -q -fdx
}
fetch_git "$src/virglrenderer" https://gitlab.freedesktop.org/virgl/virglrenderer.git \
  "$VIRGLRENDERER_VERSION" "$VIRGLRENDERER_COMMIT"
fetch_git "$src/vulkan-headers" https://github.com/KhronosGroup/Vulkan-Headers.git \
  "$VULKAN_HEADERS_TAG" "$VULKAN_HEADERS_COMMIT"

# Meson from QEMU's own tarball, so both builds use one pinned version.
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install -q --no-index "$src/qemu-$QEMU_VERSION"/python/wheels/meson-*.whl
export PATH="$work/venv/bin:$PATH"
# Venus needs Vulkan headers newer than some distributions ship; the loader
# stays the host's.
mkdir -p "$prefix/include"
cp -r "$src/vulkan-headers/include/vulkan" "$src/vulkan-headers/include/vk_video" "$prefix/include/"
export CFLAGS="-O2 -I$prefix/include"
export PKG_CONFIG_PATH="$prefix/lib/pkgconfig:$prefix/lib/$arch-linux-gnu/pkgconfig"

rm -rf "$work/virgl-build"
meson setup "$work/virgl-build" "$src/virglrenderer" --prefix="$prefix" --libdir=lib \
  --buildtype=release -Dvenus=true -Dplatforms=egl -Dtests=false -Dvideo=false
ninja -C "$work/virgl-build" install

rm -rf "$work/qemu-build"
mkdir -p "$work/qemu-build"
(cd "$work/qemu-build" && "$src/qemu-$QEMU_VERSION/configure" --prefix="$prefix" \
  --target-list="$target" --enable-kvm --enable-virglrenderer --enable-opengl --enable-gbm \
  --enable-slirp --enable-seccomp --enable-cap-ng --enable-vnc --enable-tools \
  --disable-docs --disable-gtk --disable-sdl --disable-spice --disable-xen \
  --disable-werror)
make -C "$work/qemu-build" -j"$(( ($(nproc) + 1) / 2 ))"
make -C "$work/qemu-build" install

mkdir -p "$stage/bin" "$stage/lib" "$stage/libexec" "$stage/share"
cp "$prefix/bin/$emulator" "$prefix/bin/qemu-img" "$stage/bin/"
# Venus runs each guest context in virglrenderer's render server, a program of
# its own; the service finds it through RENDER_SERVER_EXEC_PATH.
cp "$prefix/libexec/virgl_render_server" "$stage/libexec/"
cp -r "$prefix/share/qemu" "$stage/share/"
# The libraries to ship: everything the binaries load except glibc and the host
# driver's user-space.
host_owned='^(linux-vdso|ld-linux|libc|libm|libpthread|libdl|librt|libresolv|libgbm|libEGL|libGL|libGLX|libGLdispatch|libOpenGL|libvulkan|libdrm)[.-]'
for b in "$stage/bin/"* "$stage/libexec/"*; do LD_LIBRARY_PATH="$prefix/lib" ldd "$b"; done | awk '$2 == "=>" && $3 ~ /^\// { print $1, $3 }' | sort -u |
  while read -r name path; do
    if ! printf '%s\n' "$name" | grep -Eq "$host_owned"; then cp -L "$path" "$stage/lib/$name"; fi
  done
# Every binary and library finds the shipped libraries next to it.
# shellcheck disable=SC2016
for f in "$stage/bin/"* "$stage/libexec/"* "$stage/lib/"*; do patchelf --set-rpath '$ORIGIN/../lib' "$f"; done

# The oldest glibc any shipped file needs; the updater compares it with the computer's.
glibc_min="$(for f in "$stage/bin/"* "$stage/libexec/"* "$stage/lib/"*; do objdump -T "$f"; done |
  grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -t. -k2,2n -u | tail -n 1 | sed 's/GLIBC_//')"
"$stage/bin/$emulator" -device help | grep -q '"virtio-gpu-gl-pci"' ||
  { echo "the build has no virtio-gpu-gl-pci" >&2; exit 1; }
"$stage/bin/$emulator" -device virtio-gpu-gl-pci,help | grep -q 'venus' ||
  { echo "the build's virtio-gpu-gl-pci has no venus property" >&2; exit 1; }
cat >"$stage/VERSIONS" <<EOF
label=$label
qemu=$QEMU_VERSION
virglrenderer=$VIRGLRENDERER_VERSION
venus=true
render_server=libexec/virgl_render_server
emulator=$emulator
glibc_min=$glibc_min
EOF
cp "$src/qemu-$QEMU_VERSION/COPYING" "$stage/COPYING.qemu"
cp "$src/virglrenderer/COPYING" "$stage/COPYING.virglrenderer"

name="flip-qemu-$label-$arch.tar.gz"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$stage" -czf "$out/$name" .
(cd "$out" && sha256sum "$name" > "$name.sha256")
cat "$out/$name.sha256"
