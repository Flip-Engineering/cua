// Flip's additions to gVisor's NVIDIA driver table. build.sh copies this file
// into pkg/sentry/devices/nvproxy and calls addFlipDriverABIs at the end of
// Init's table construction.

package nvproxy

import (
	"fmt"

	"gvisor.dev/gvisor/pkg/sentry/devices/nvproxy/nvconf"
)

// flipDriverABI is one NVIDIA driver version that uses the ABI of base.
type flipDriverABI struct {
	version   nvconf.DriverVersion
	base      nvconf.DriverVersion
	checksums Checksums
}

// addFlipDriverABIs adds Flip's driver versions to abis, then lets the other
// additions of this build (flipABIWrappers) wrap every ABI. It runs once, at
// the end of Init's table construction.
func addFlipDriverABIs() {
	for _, d := range flipDriverABIs {
		if _, ok := abis[d.version]; ok {
			continue // gVisor knows this version itself.
		}
		base, ok := abis[d.base]
		if !ok {
			panic(fmt.Sprintf("nvproxy: Flip driver %s names base %s, which this gVisor does not know", d.version, d.base))
		}
		abis[d.version] = driverABIEntry{cons: base.cons, checksums: d.checksums, supported: base.supported}
	}
	for _, wrap := range flipABIWrappers {
		for v, e := range abis {
			e.cons = wrap(e.cons)
			abis[v] = e
		}
	}
}

// flipABIWrappers change every driver ABI of this build; each file that adds
// one registers it in an init function.
var flipABIWrappers []func(driverABIFunc) driverABIFunc
