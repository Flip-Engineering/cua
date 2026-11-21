#!/usr/bin/env bash
# SPDX-License-Identifier: FSL-1.1-MIT
# Copyright (c) 2026 Cua AI, Inc.

# Compile on the x64 build host. Use QEMU only for the release smoke tests.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
target=$1
packages=(python3-venv python3-pip)
case "$target" in
  aarch64-unknown-linux-gnu)
    dpkg --add-architecture arm64
    packages+=(binutils-aarch64-linux-gnu libc6-arm64-cross libgcc-s1-arm64-cross
      libx11-dev:arm64 libx11-xcb-dev:arm64 libxcb1-dev:arm64 libxkbcommon-dev:arm64
      libxkbcommon-x11-dev:arm64 libxrandr-dev:arm64 libxi-dev:arm64 libxtst-dev:arm64
      libxext-dev:arm64 libxfixes-dev:arm64 libxdamage-dev:arm64 libwayland-dev:arm64
      libdbus-1-dev:arm64 libudev-dev:arm64)
    apt-get update
    ;;
  aarch64-unknown-linux-musl) ;;
  *) printf 'Unsupported cross-compilation target %s\n' "$target" >&2; exit 1 ;;
esac
apt-get install -y --no-install-recommends "${packages[@]}"

tools=$(mktemp -d "${RUNNER_TEMP:?}/cua-zigbuild.XXXXXX")
version=0.22.3
archive=cargo-zigbuild-x86_64-unknown-linux-musl.tar.xz
curl --fail --silent --show-error --location --retry 3 --retry-all-errors \
  "https://github.com/rust-cross/cargo-zigbuild/releases/download/v$version/$archive" \
  -o "$tools/$archive"
(
  cd "$tools"
  printf '%s  %s\n' 9fd7d8136cbf7ab2ae44c046f4985c7a1aa6e2ed483187396df0076bedfa196b "$archive" |
    sha256sum --check
  tar --no-same-owner -xf "$archive" --strip-components=1 --wildcards '*/cargo-zigbuild'
)
python3 -m venv "$tools/zig"
printf '%s\n' 'ziglang==0.15.2 --hash=sha256:806316453d1ede7174ed3fefb8e5348154629089c3bb5fe812dfd0e172fac130' \
  > "$tools/requirements.txt"
"$tools/zig/bin/python" -m pip install --disable-pip-version-check --require-hashes --only-binary=:all: \
  -r "$tools/requirements.txt"
ln -s "$tools/zig/lib/"python*/site-packages/ziglang/zig "$tools/zig/bin/zig"
"$tools/zig/bin/zig" version
"$tools/cargo-zigbuild" --version
printf '%s\n' "$tools" "$tools/zig/bin" >> "${GITHUB_PATH:?}"

if [[ "$target" == aarch64-unknown-linux-gnu ]]; then
  {
    echo 'PKG_CONFIG_ALLOW_CROSS_aarch64_unknown_linux_gnu=1'
    echo 'PKG_CONFIG_LIBDIR_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig'
    echo 'CFLAGS_aarch64_unknown_linux_gnu=-idirafter /usr/include/aarch64-linux-gnu -idirafter /usr/include'
    # audiopus_sys builds an archive from its vendored source when pkg-config has no Opus.
    echo 'LIBOPUS_STATIC=1'
    echo 'QEMU_LD_PREFIX=/usr/aarch64-linux-gnu'
    echo 'OBJDUMP=aarch64-linux-gnu-objdump'
  } >> "${GITHUB_ENV:?}"
fi
