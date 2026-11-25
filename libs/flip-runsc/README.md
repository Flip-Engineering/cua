# Flip's runsc

gVisor's `runsc` at the release in `versions.env`, built from gVisor's `go` branch with plain
`go build`, plus two additions in `version_flip.go`:

- NVIDIA driver versions that gVisor's table lacks but whose kernel interface equals a version it
  knows (`drivers.json`, written by `driver_table.py scan`). A version is accepted only when no
  header of NVIDIA's open kernel modules changed between it and its known lower neighbour except
  the version and product-name lines. It then uses that neighbour's ABI, exactly as gVisor chains
  such versions itself.
- The Volta classes, each with the handler of the Turing class of the same kind.

Refresh the driver list (it fetches the NVIDIA tags it compares into the repository you name, and
downloads each accepted release's installer to record its SHA-256):

    git init nvidia && git -C nvidia remote add origin https://github.com/NVIDIA/open-gpu-kernel-modules.git
    python3 driver_table.py scan --checksums --version-go <gvisor>/pkg/sentry/devices/nvproxy/version.go \
      --nvidia-repo nvidia --out drivers.json

`build.sh <amd64|arm64> <work dir> <out dir>` builds `flip-runsc-<label>-<arch>.tar.gz`; the
`runsc-linux` job of `cd-cua-spacesd.yml` runs it for each release.
