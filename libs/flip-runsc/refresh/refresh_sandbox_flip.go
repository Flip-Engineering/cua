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

// Flip's runsc build adds this file to runsc/sandbox.

package sandbox

import (
	"fmt"

	"gvisor.dev/gvisor/runsc/boot/sentryapi"
)

// RefreshNvproxyDevices asks the sentry to make container cid's GPU device
// files equal to the cards its device gofer has now.
func (s *Sandbox) RefreshNvproxyDevices(cid string) (*sentryapi.RefreshNvproxyReply, error) {
	var reply sentryapi.RefreshNvproxyReply
	if err := s.call(sentryapi.ContMgrRefreshNvproxyDevices, &cid, &reply); err != nil {
		return nil, fmt.Errorf("refreshing the GPU devices of container %q: %w", cid, err)
	}
	return &reply, nil
}
