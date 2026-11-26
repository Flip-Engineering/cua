// Copyright 2026 The gVisor Authors.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Flip's runsc build adds this file to runsc/boot.

package boot

import (
	"fmt"

	"gvisor.dev/gvisor/pkg/abi/linux"
	"gvisor.dev/gvisor/pkg/abi/nvgpu"
	"gvisor.dev/gvisor/pkg/fspath"
	"gvisor.dev/gvisor/pkg/log"
	"gvisor.dev/gvisor/pkg/sentry/fsimpl/dev"
	"gvisor.dev/gvisor/pkg/sentry/kernel/auth"
	"gvisor.dev/gvisor/pkg/sentry/vfs"
	"gvisor.dev/gvisor/runsc/boot/sentryapi"
)

// RefreshNvproxyDevices makes the GPU device files of running container cid
// equal to the cards its device gofer lists. A card the host gained while the
// container runs gets /dev/nvidia<N> (the GPU proxy registered every card
// number at start and opens the host's file through the device gofer when a
// program opens it); a card that left loses its file.
func (cm *containerManager) RefreshNvproxyDevices(cid *string, reply *sentryapi.RefreshNvproxyReply) error {
	log.Debugf("containerManager.RefreshNvproxyDevices, cid: %s", *cid)
	return cm.l.refreshNvproxyDevices(*cid, reply)
}

func (l *Loader) refreshNvproxyDevices(cid string, reply *sentryapi.RefreshNvproxyReply) error {
	l.mu.Lock()
	defer l.mu.Unlock()
	contName := ""
	for name, id := range l.containerIDs {
		if id == cid {
			contName = name
			break
		}
	}
	if contName == "" {
		return fmt.Errorf("container %q is not in this sandbox", cid)
	}
	ctx := l.k.SupervisorContext()
	devClient := l.k.GetDevGoferClient(contName)
	if devClient == nil {
		return fmt.Errorf("container %q has no device gofer: its GPU proxy was not set up at its start", cid)
	}
	present, err := nvidiaRegularDeviceMinorsFromGofer(ctx, devClient)
	if err != nil {
		return err
	}
	ep, ok := l.processes[execID{cid: cid}]
	if !ok || ep.tg == nil {
		return fmt.Errorf("container %q has no root process", cid)
	}
	mntns := ep.tg.Leader().GetMountNamespace()
	if mntns == nil {
		return fmt.Errorf("container %q has ended", cid)
	}
	defer mntns.DecRef(ctx)
	root := mntns.Root(ctx)
	if !root.Ok() {
		return fmt.Errorf("container %q has no root", cid)
	}
	defer root.DecRef(ctx)
	vfsObj := l.k.VFS()
	creds := auth.CredentialsFromContext(ctx)
	pop := func(minor uint32) *vfs.PathOperation {
		return &vfs.PathOperation{Root: root, Start: root, Path: fspath.Parse(nvidiaDevicePath(minor))}
	}
	exists := func(minor uint32) bool {
		st, err := vfsObj.StatAt(ctx, creds, pop(minor), &vfs.StatOptions{})
		return err == nil && st.Mode&linux.S_IFMT == linux.S_IFCHR && st.RdevMajor == nvgpu.NV_MAJOR_DEVICE_NUMBER && st.RdevMinor == minor
	}
	add, remove := planNvproxyRefresh(present, exists)
	for _, minor := range add {
		if err := dev.CreateDeviceFile(ctx, vfsObj, creds, root, nvidiaDevicePath(minor), nvgpu.NV_MAJOR_DEVICE_NUMBER, minor, linux.FileMode(linux.S_IFCHR|0o666), nil /* uid */, nil /* gid */); err != nil {
			return fmt.Errorf("creating %s: %w", nvidiaDevicePath(minor), err)
		}
		reply.Added = append(reply.Added, minor)
	}
	for _, minor := range remove {
		if err := vfsObj.UnlinkAt(ctx, creds, pop(minor)); err != nil {
			return fmt.Errorf("removing %s: %w", nvidiaDevicePath(minor), err)
		}
		reply.Removed = append(reply.Removed, minor)
	}
	return nil
}

func nvidiaDevicePath(minor uint32) string {
	return fmt.Sprintf("/dev/nvidia%d", minor)
}

// planNvproxyRefresh returns the card numbers whose device file must be made
// (in present, not existing) and removed (existing, not in present), over the
// whole range of regular NVIDIA card numbers, in increasing order.
func planNvproxyRefresh(present []uint32, exists func(uint32) bool) (add, remove []uint32) {
	want := make(map[uint32]bool, len(present))
	for _, m := range present {
		want[m] = true
	}
	for m := uint32(0); m <= nvgpu.NV_MINOR_DEVICE_NUMBER_REGULAR_MAX; m++ {
		has := exists(m)
		switch {
		case want[m] && !has:
			add = append(add, m)
		case !want[m] && has:
			remove = append(remove, m)
		}
	}
	return add, remove
}
