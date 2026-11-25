# Flip's runsc

gVisor's `runsc` at the release in `versions.env`, built from gVisor's `go` branch with plain
`go build`, plus two additions in `version_flip.go`:

- NVIDIA driver versions that gVisor's table lacks but whose kernel interface equals a version it
  knows (`drivers.json`, written by `driver_table.py scan`). A version is accepted only when no
  header of NVIDIA's open kernel modules changed between it and its known lower neighbour except
  the version and product-name lines. It then uses that neighbour's ABI, exactly as gVisor chains
  such versions itself.
- The Volta classes (`version_flip_volta.go`), each with the handler of the Turing class of the
  same kind. Untested on a card until the first real run on a V100: `untested.json` lists the
  family's device IDs, DRIVERS.json and UNTESTED.txt carry them, and the machine report says
  "untested on this card family". `VOLTA-SOURCES.md` puts NVIDIA's class header lines beside
  gVisor's Turing handlers.

Refresh the driver list (it fetches the NVIDIA tags it compares into the repository you name, and
downloads each accepted release's installer to record its SHA-256):

    git init nvidia && git -C nvidia remote add origin https://github.com/NVIDIA/open-gpu-kernel-modules.git
    python3 driver_table.py scan --checksums --version-go <gvisor>/pkg/sentry/devices/nvproxy/version.go \
      --nvidia-repo nvidia --out drivers.json

The release gate compares every accepted version with its base again and fails when an
interface header changed; its notes go into the release notes:

    python3 driver_table.py verify --version-go <version.go> --nvidia-repo nvidia \
      --drivers drivers.json --notes DRIVER-NOTES.md

NVIDIA's security bulletins for its Linux driver (github.com/NVIDIA/product-security) are data:
`bulletins.py refresh --out bulletins.json` reads them, the release job refreshes them, and the
tarball carries them as DRIVERS.json `bulletins` and BULLETINS.txt. Flip's updater compares the
computer's driver with them and reports the fixed version of its branch; Flip tells the owner
once. Nothing installs a driver.

`build.sh <amd64|arm64> <work dir> <out dir>` builds `flip-runsc-<label>-<arch>.tar.gz`; the
`runsc-linux` job of `cd-cua-spacesd.yml` runs it for each release.
