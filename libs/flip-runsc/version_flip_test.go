package nvproxy

import (
	"reflect"
	"testing"

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
