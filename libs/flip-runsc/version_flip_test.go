package nvproxy

import (
	"reflect"
	"testing"

	"gvisor.dev/gvisor/pkg/abi/nvgpu"
	"gvisor.dev/gvisor/pkg/sentry/devices/nvproxy/nvconf"
)

// A Flip driver version proxies exactly the ioctls, commands and classes of its
// base version, and nvproxy starts for it.
func TestFlipDriverHasItsBaseABI(t *testing.T) {
	Init()
	supported := map[nvconf.DriverVersion]bool{}
	for _, v := range SupportedDrivers() {
		supported[v] = true
	}
	for _, d := range flipDriverABIs {
		if !supported[d.version] {
			t.Errorf("%s is not a supported driver", d.version)
		}
		fe, uvm, ctl, alloc, ok := SupportedIoctlsNumbers(d.version)
		bfe, buvm, bctl, balloc, bok := SupportedIoctlsNumbers(d.base)
		if !ok || !bok {
			t.Fatalf("%s or its base %s has no ABI", d.version, d.base)
		}
		for name, pair := range map[string][2]map[uint32]struct{}{
			"frontend": {fe, bfe}, "uvm": {uvm, buvm}, "control": {ctl, bctl}, "alloc": {alloc, balloc},
		} {
			if !reflect.DeepEqual(pair[0], pair[1]) {
				t.Errorf("%s %s ioctls differ from base %s", d.version, name, d.base)
			}
		}
	}
}

// Every ABI that allocates a Turing class allocates its Volta class with the
// same handler, and Volta's usermode class keeps its graphics capabilities.
func TestVoltaClassesFollowTuring(t *testing.T) {
	Init()
	for v, e := range abis {
		abi := e.cons()
		for _, c := range voltaFromTuring {
			turing, ok := abi.allocationClass[c.turing]
			if !ok {
				continue
			}
			volta, ok := abi.allocationClass[c.volta]
			if !ok {
				t.Errorf("%s: %s missing", v, c.name)
				continue
			}
			if reflect.ValueOf(volta.handler).Pointer() != reflect.ValueOf(turing.handler).Pointer() {
				t.Errorf("%s: %s handler differs from its Turing class", v, c.name)
			}
			if volta.capSet&turing.capSet != turing.capSet {
				t.Errorf("%s: %s lacks capabilities of its Turing class", v, c.name)
			}
		}
		if um, ok := abi.allocationClass[nvgpu.VOLTA_USERMODE_A]; ok {
			if um.capSet&(nvconf.CapGraphics|nvconf.CapVideo) != nvconf.CapGraphics|nvconf.CapVideo {
				t.Errorf("%s: VOLTA_USERMODE_A lost graphics or video", v)
			}
		}
		info := abi.getInfo()
		for _, c := range voltaFromTuring {
			if _, ok := abi.allocationClass[c.volta]; ok {
				if got := info.AllocationInfos[c.volta].Name; got != c.name {
					t.Errorf("%s: info for %#x is %q, want %q", v, uint32(c.volta), got, c.name)
				}
			}
		}
	}
}
