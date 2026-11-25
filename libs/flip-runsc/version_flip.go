// Flip's additions to gVisor's NVIDIA driver table. build.sh copies this file
// into pkg/sentry/devices/nvproxy and calls addFlipDriverABIs at the end of
// Init's table construction.

package nvproxy

import (
	"fmt"

	"gvisor.dev/gvisor/pkg/abi/nvgpu"
	"gvisor.dev/gvisor/pkg/sentry/devices/nvproxy/nvconf"
)

// flipDriverABI is one NVIDIA driver version that uses the ABI of base.
type flipDriverABI struct {
	version   nvconf.DriverVersion
	base      nvconf.DriverVersion
	checksums Checksums
}

// Volta classes from NVIDIA's open headers (src/common/sdk/nvidia/inc/class):
// clc36f.h VOLTA_CHANNEL_GPFIFO_A, clc3c0.h VOLTA_COMPUTE_A, clc3b5.h
// VOLTA_DMA_COPY_A, clc397.h VOLTA_A. The driver allocates each with the same
// parameter struct as the Turing class of the same kind
// (NV_CHANNEL_ALLOC_PARAMS, NV_GR_ALLOCATION_PARAMETERS,
// NVB0B5_ALLOCATION_PARAMETERS), so each gets the Turing class's handler in
// every driver ABI. gVisor's table has VOLTA_USERMODE_A for graphics and video
// only; CUDA on a Volta card maps the doorbell through it, so it also gets
// TURING_USERMODE_A's capabilities.
const (
	voltaChannelGPFIFOA nvgpu.ClassID = 0x0000c36f
	voltaComputeA       nvgpu.ClassID = 0x0000c3c0
	voltaDMACopyA       nvgpu.ClassID = 0x0000c3b5
	voltaA              nvgpu.ClassID = 0x0000c397
)

var voltaFromTuring = []struct {
	volta, turing nvgpu.ClassID
	name          string
}{
	{voltaChannelGPFIFOA, nvgpu.TURING_CHANNEL_GPFIFO_A, "VOLTA_CHANNEL_GPFIFO_A"},
	{voltaComputeA, nvgpu.TURING_COMPUTE_A, "VOLTA_COMPUTE_A"},
	{voltaDMACopyA, nvgpu.TURING_DMA_COPY_A, "VOLTA_DMA_COPY_A"},
	{voltaA, nvgpu.TURING_A, "VOLTA_A"},
	{nvgpu.VOLTA_USERMODE_A, nvgpu.TURING_USERMODE_A, "VOLTA_USERMODE_A"},
}

// withVolta returns cons whose ABI also allocates the Volta classes.
func withVolta(cons driverABIFunc) driverABIFunc {
	return func() *driverABI {
		abi := cons()
		for _, c := range voltaFromTuring {
			turing, ok := abi.allocationClass[c.turing]
			if !ok {
				continue
			}
			h := turing
			if existing, ok := abi.allocationClass[c.volta]; ok {
				h.capSet |= existing.capSet
			}
			abi.allocationClass[c.volta] = h
		}
		prevGetInfo := abi.getInfo
		abi.getInfo = func() *DriverABIInfo {
			info := prevGetInfo()
			for _, c := range voltaFromTuring {
				if turing, ok := info.AllocationInfos[c.turing]; ok {
					info.AllocationInfos[c.volta] = IoctlInfo{Name: c.name, Structs: turing.Structs}
				}
			}
			return info
		}
		return abi
	}
}

// addFlipDriverABIs adds Flip's driver versions and the Volta classes to abis.
// It runs once, at the end of Init's table construction.
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
	for v, e := range abis {
		e.cons = withVolta(e.cons)
		abis[v] = e
	}
}
