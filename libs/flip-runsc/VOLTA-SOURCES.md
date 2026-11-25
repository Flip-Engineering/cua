# Volta classes: the source lines behind version_flip_volta.go

**Untested on a card.** Nobody has run CUDA through these classes on a Volta card yet. Until a
real take passes, DRIVERS.json and untested.json mark the family "untested", and the machine
report says "untested on this card family" for every device ID in untested.json.

Sources: NVIDIA open-gpu-kernel-modules at tag `580.126.16`; gVisor
`pkg/sentry/devices/nvproxy/version.go` at release `release-20261123.0` (commit 6abe42f82).

## Class numbers: NVIDIA's headers, Volta beside Turing

| Volta class (NVIDIA header) | Turing class of the same kind (NVIDIA header) |
| --- | --- |
| `class/clc36f.h:43` `#define  VOLTA_CHANNEL_GPFIFO_A (0x0000C36F)` | `class/clc46f.h:43` `#define  TURING_CHANNEL_GPFIFO_A (0x0000C46F)` |
| `class/clc3c0.h:32` `#define VOLTA_COMPUTE_A    0xC3C0` | `class/clc5c0.h:32` `#define TURING_COMPUTE_A    0xC5C0` |
| `class/clc3b5.h:33` `#define VOLTA_DMA_COPY_A (0x0000C3B5)` | `class/clc5b5.h:33` `#define TURING_DMA_COPY_A (0x0000C5B5)` |
| `class/clc397.h:32` `#define VOLTA_A    0xC397` | `class/clc597.h:32` `#define TURING_A    0xC597` |
| `class/clc361.h:27` `#define VOLTA_USERMODE_A (0xc361)` | `class/clc461.h:27` `#define TURING_USERMODE_A (0xc461)` |

Paths are under `src/common/sdk/nvidia/inc/`.

## Allocation parameters: NVIDIA's resource table

`src/nvidia/src/kernel/rmapi/resource_list.h` names the parameter struct of each class. The
open module's resource manager supports Turing and later cards, so it lists only two Volta
classes. For the other three, the table shows the same struct for every class of the kind,
also for Pascal, the generation before Volta:

| Kind | Volta entry | Turing entry | Every other entry of the kind |
| --- | --- | --- | --- |
| Channel | `:331` VOLTA_CHANNEL_GPFIFO_A `RS_REQUIRED(NV_CHANNEL_ALLOC_PARAMS)` | `:342` TURING_CHANNEL_GPFIFO_A `RS_REQUIRED(NV_CHANNEL_ALLOC_PARAMS)` | GF100 to BLACKWELL_B (`:277` to `:386`): the same |
| User mode | `:857` VOLTA_USERMODE_A `RS_NONE` | `:868` TURING_USERMODE_A `RS_NONE` | AMPERE `RS_NONE`; HOPPER and BLACKWELL `NV_HOPPER_USERMODE_A_PARAMS` |
| Copy | not listed | `:1584` TURING_DMA_COPY_A `RS_OPTIONAL(NVB0B5_ALLOCATION_PARAMETERS)` | MAXWELL `:1562`, PASCAL `:1573` and AMPERE to BLACKWELL: the same |
| Compute | not listed | `:2161` TURING_COMPUTE_A `RS_OPTIONAL(NV_GR_ALLOCATION_PARAMETERS)` | AMPERE to BLACKWELL (`:2000` to `:2118`): the same |
| 3D | not listed | `:2151` TURING_A `RS_OPTIONAL(NV_GR_ALLOCATION_PARAMETERS)` | AMPERE to BLACKWELL, FERMI_TWOD_A (`:2129`): the same |

The proprietary module, which drives Volta cards, has no public resource table. The Copy,
Compute and 3D rows are therefore an inference from the uniform pattern; the first real run
checks them.

## gVisor's handlers that the Volta classes copy

`version.go` at the release, the base ABI (`v535_104_05` constructor):

| Line | gVisor entry | Used for |
| --- | --- | --- |
| 514 | `nvgpu.VOLTA_USERMODE_A: allocHandler(rmAllocNoParams, nvconf.CapGraphics\|nvconf.CapVideo)` | kept; `withVolta` adds TURING_USERMODE_A's capabilities |
| 515 | `nvgpu.TURING_CHANNEL_GPFIFO_A: allocHandler(rmAllocChannel, compUtil)` | VOLTA_CHANNEL_GPFIFO_A |
| 533 | `nvgpu.TURING_A: allocHandler(rmAllocSimple[nvgpu.NV_GR_ALLOCATION_PARAMETERS], nvconf.CapGraphics)` | VOLTA_A |
| 538 | `nvgpu.TURING_DMA_COPY_A: allocHandler(rmAllocSimple[nvgpu.NVB0B5_ALLOCATION_PARAMETERS], compUtil)` | VOLTA_DMA_COPY_A |
| 542 | `nvgpu.TURING_COMPUTE_A: allocHandler(rmAllocSimple[nvgpu.NV_GR_ALLOCATION_PARAMETERS], compUtil)` | VOLTA_COMPUTE_A |
| 550 | `nvgpu.TURING_USERMODE_A: allocHandler(rmAllocNoParams, compUtil)` | VOLTA_USERMODE_A (capabilities) |

The info table for `runsc nvproxy` (lines 881 to 917) names the same structs, and
`withVolta` copies them under the Volta names. Later ABIs replace the Turing channel handler
(the V570 and V610 overrides at lines 1200 and 1398); `withVolta` wraps every constructor, so the Volta channel class
always takes the handler that the Turing class has in that ABI.

## Device IDs

`untested.json` lists the NVIDIA device IDs whose chip name in pci.ids starts with `GV`
(`driver_table.py untested --pci-ids pci.ids --out untested.json`).
