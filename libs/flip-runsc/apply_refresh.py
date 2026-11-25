#!/usr/bin/env python3
"""Adds Flip's GPU device refresh to a gVisor source tree (build.sh calls it).

    apply_refresh.py <gvisor source dir>

1. runsc's device rule allows every card number of the NVIDIA major whenever
   the GPU proxy is on for the sandbox (CDI devices included), not only on the
   legacy hook path, so a card that appears while a Space runs can be opened.
2. The control call containerManager.RefreshNvproxyDevices and the command
   `runsc nvproxy refresh <container id>` make the container's /dev/nvidia<N>
   files equal to the cards its device gofer has.
"""
import shutil
import sys
from pathlib import Path

here = Path(__file__).resolve().parent
src = Path(sys.argv[1])

def replace_once(path, old, new):
    text = path.read_text()
    if text.count(old) != 1:
        sys.exit(f"{path}: expected the anchor exactly once: {old!r}")
    path.write_text(text.replace(old, new))

replace_once(src / "runsc/container/devices.go",
    "\tif specutils.GPUFunctionalityRequestedViaHook(spec, conf) {\n\t\t// nvidia-container-cli",
    "\tif specutils.NVProxyEnabled(spec, conf) {\n\t\t// nvidia-container-cli")
replace_once(src / "runsc/cmd/nvproxy/nvproxy.go",
    "\tcdr.Register(&listSupportedDrivers{supportedDrivers: n.SupportedDrivers}, \"\")\n",
    "\tcdr.Register(&listSupportedDrivers{supportedDrivers: n.SupportedDrivers}, \"\")\n\tcdr.Register(&refresh{}, \"\")\n")
for name, dest in [
    ("refresh_sentryapi_flip.go", "runsc/boot/sentryapi/refresh_flip.go"),
    ("nvproxy_refresh_flip.go", "runsc/boot/nvproxy_refresh_flip.go"),
    ("nvproxy_refresh_flip_test.go", "runsc/boot/nvproxy_refresh_flip_test.go"),
    ("refresh_sandbox_flip.go", "runsc/sandbox/refresh_flip.go"),
    ("refresh_cmd_flip.go", "runsc/cmd/nvproxy/refresh_flip.go"),
]:
    shutil.copyfile(here / "refresh" / name, src / dest)
