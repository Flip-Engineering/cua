#!/usr/bin/env bash
# Builds Flip's runsc: gVisor at the pinned release, plus the NVIDIA driver
# versions in drivers.json and the Volta classes (version_flip.go).
#
#   build.sh <amd64|arm64> <work dir> <out dir>
#
# The work dir holds the gVisor checkout and Go's caches; it must be on a disk
# that keeps its contents (never /tmp). The out dir gets
# flip-runsc-<label>-<x86_64|aarch64>.tar.gz with the layout of gVisor's own release
# tarball (runsc, containerd-shim-runsc-v1, gvisor-bin/) plus DRIVERS.json,
# UNTESTED.txt, BULLETINS.txt, RUNTIME_ARGS and LICENSE. Needs git, go, python3 and a C
# compiler for the target ($CC, default gcc for amd64 and aarch64-linux-gnu-gcc
# for arm64).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
arch="${1:?arch}"; work="${2:?work dir}"; out="${3:?out dir}"
# shellcheck source=versions.env
source "$here/versions.env"
case "$arch" in
  amd64) triple=x86_64; cc="${CC:-gcc}" ;;
  arm64) triple=aarch64; cc="${CC:-aarch64-linux-gnu-gcc}" ;;
  *) echo "unknown arch $arch" >&2; exit 2 ;;
esac
label="$GVISOR_RELEASE-flip.$FLIP_RUNSC_BUILD"
src="$work/gvisor-$GVISOR_GO_COMMIT"
mkdir -p "$work" "$out"

if [ ! -d "$src/.git" ]; then
  git init -q "$src"
  git -C "$src" remote add origin https://github.com/google/gvisor.git
fi
git -C "$src" fetch -q --depth 2 origin "$GVISOR_GO_COMMIT"
git -C "$src" fetch -q --depth 1 origin "refs/tags/$GVISOR_RELEASE:refs/tags/$GVISOR_RELEASE"
git -C "$src" checkout -q --force --detach "$GVISOR_GO_COMMIT"
git -C "$src" clean -q -fdx
# The go branch commit must merge exactly the release's commit.
merged="$(git -C "$src" rev-parse "$GVISOR_GO_COMMIT^2")"
tagged="$(git -C "$src" rev-parse "refs/tags/$GVISOR_RELEASE^{commit}")"
if [ "$merged" != "$GVISOR_RELEASE_COMMIT" ] || [ "$tagged" != "$GVISOR_RELEASE_COMMIT" ]; then
  echo "go commit merges $merged, tag names $tagged, want $GVISOR_RELEASE_COMMIT" >&2
  exit 1
fi

gomod="$(sed -n 's/^go //p' "$src/go.mod")"
gover="$(cd "$src" && go env GOVERSION)"
if [ "$gomod" != "$GO_VERSION" ] || [ "$gover" != "go$GO_VERSION" ]; then
  echo "gVisor wants Go $gomod, versions.env names $GO_VERSION, this Go is $gover" >&2
  exit 1
fi

# Changes to gVisor's own files are patches under patches/, applied in name
# order to the pinned commit; each must apply cleanly or the build stops.
for p in "$here"/patches/*.patch; do
  [ -e "$p" ] || continue
  git -C "$src" apply --check "$p"
  git -C "$src" apply "$p"
done
nv="$src/pkg/sentry/devices/nvproxy"
cp "$here"/version_flip*.go "$nv/"
# riff-one-vm's GPU device refresh: every card number of the NVIDIA major is
# allowed under the proxy, and `runsc nvproxy refresh <cid>` brings a running
# Space's /dev/nvidia<N> to the cards its gofer has.
python3 "$here/apply_refresh.py" "$src"
python3 "$here/driver_table.py" go --drivers "$here/drivers.json" --out "$nv/version_flip_drivers.go"
# Init builds the table inside abisOnce.Do; a deferred call at the top of that
# function runs after the last entry is added.
python3 - "$nv/version.go" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
anchor = "func Init() {\n\tabisOnce.Do(func() {\n"
if text.count(anchor) != 1:
    sys.exit("version.go: Init's table construction not found exactly once")
open(path, "w").write(text.replace(anchor, anchor + "\t\tdefer addFlipDriverABIs()\n"))
PY
gofmt -l "$nv" | (! grep .)

export CGO_ENABLED=0 GOFLAGS=-trimpath GOCACHE="$work/go-cache" GOMODCACHE="$work/go-mod"
(cd "$src" && go test -count=1 ./pkg/sentry/devices/nvproxy/ && go test -count=1 -run Flip ./runsc/boot/)

stage="$work/stage-$triple"
rm -rf "$stage" && mkdir -p "$stage/gvisor-bin"
ld="-s -w -X gvisor.dev/gvisor/runsc/version.version=$label"
build() { (cd "$src" && GOOS=linux GOARCH="$arch" go build -ldflags "$ld" -o "$1" "$2"); }
build "$stage/runsc" ./runsc
build "$stage/containerd-shim-runsc-v1" ./shim
build "$stage/gvisor-bin/gvisor_sentry" ./runsc/cmd/sentry
build "$stage/gvisor-bin/runsc-netgofer" ./runsc/cmd/netgofer
build "$stage/gvisor-bin/runsc-metric-server" ./runsc/cmd/metricserver
build "$stage/gvisor-bin/checkpointgofer" ./runsc/checkpointgofer
# The three small sidecars, with the flags of their BUILD genrules.
small="-Os -ffreestanding -fno-sanitize=all -fno-stack-protector -fno-asynchronous-unwind-tables
  -fno-unwind-tables -static -nostdlib -nostartfiles -Wl,--build-id=none -Wl,--gc-sections -Wl,-n -Wl,-s"
# shellcheck disable=SC2086
"$cc" $small -fno-pie -no-pie -o "$stage/gvisor-bin/runsc-fd-parking" "$src/runsc/fdparking/fdparking.c"
# shellcheck disable=SC2086
"$cc" $small -no-pie -o "$stage/gvisor-bin/gvisor-proc-umounter" "$src/runsc/procumounter/procumounter.c"
# shellcheck disable=SC2086
"$cc" $small -no-pie -o "$stage/gvisor-bin/gvisor-sentry-prewarmer" "$src/runsc/prewarmer/prewarmer.c"
cp "$src/LICENSE" "$stage/LICENSE"
# The engine arguments Flip's updater registers, without comments.
grep -v '^#' "$here/runtime-args" | sed '/^$/d' >"$stage/RUNTIME_ARGS"
# The release job refreshes NVIDIA's bulletins into FLIP_RUNSC_BULLETINS; a local
# build uses the copy in this directory.
bulletins="${FLIP_RUNSC_BULLETINS:-$here/bulletins.json}"
python3 - "$here/drivers.json" "$here/untested.json" "$bulletins" "$stage" "$label" <<'PY'
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(sys.argv[1])))
import bulletins
drivers, untested, bulletin_file, stage, label = sys.argv[1:]
rows = [r for r in json.load(open(drivers)) if r["accepted"]]
families = json.load(open(untested))
news = json.load(open(bulletin_file))
# The families of untested.json have classes that have not run CUDA on a card yet.
json.dump({"label": label, "drivers": [{"version": r["version"], "base": r["base"]} for r in rows],
           "untested": families, "bulletins": news},
          open(os.path.join(stage, "DRIVERS.json"), "w"), indent=1)
# Plain lines for the updater, which runs in sh without a JSON reader.
with open(os.path.join(stage, "UNTESTED.txt"), "w") as f:
    for fam in families:
        for dev in fam["device_ids"]:
            f.write(f"{fam['vendor_id']} {dev} {fam['family']}\n")
with open(os.path.join(stage, "BULLETINS.txt"), "w") as f:
    f.write(bulletins.text(news))
PY
if [ "$arch" = "$(go env GOHOSTARCH)" ]; then
  "$stage/runsc" --version | grep -qx "runsc version $label"
  for v in $(python3 -c 'import json,sys; print(" ".join(r["version"] for r in json.load(open(sys.argv[1])) if r["accepted"]))' "$here/drivers.json"); do
    "$stage/runsc" nvproxy list-supported-drivers | grep -qx "$v" || { echo "runsc does not list $v" >&2; exit 1; }
  done
fi
# The label is in the name, so Flip's pin can name the version its updater checks.
name="flip-runsc-$label-$triple.tar.gz"
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime=@0 -C "$stage" -czf "$out/$name" .
(cd "$out" && sha256sum "$name" > "$name.sha256")
cat "$out/$name.sha256"
