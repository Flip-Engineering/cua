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
	"slices"
	"testing"
)

// A fake device directory: the card numbers that have a file now.
func fakeDev(minors ...uint32) func(uint32) bool {
	return func(m uint32) bool { return slices.Contains(minors, m) }
}

func TestFlipRefreshAddsANewCardAndRemovesAGoneOne(t *testing.T) {
	add, remove := planNvproxyRefresh([]uint32{0, 2}, fakeDev(0, 1))
	if !slices.Equal(add, []uint32{2}) || !slices.Equal(remove, []uint32{1}) {
		t.Fatalf("add %v remove %v, want [2] and [1]", add, remove)
	}
}

func TestFlipRefreshOfAnEqualSetDoesNothing(t *testing.T) {
	add, remove := planNvproxyRefresh([]uint32{0, 1}, fakeDev(0, 1))
	if len(add) != 0 || len(remove) != 0 {
		t.Fatalf("add %v remove %v, want nothing", add, remove)
	}
}

func TestFlipRefreshWithNoCardsLeftRemovesEveryFile(t *testing.T) {
	add, remove := planNvproxyRefresh(nil, fakeDev(3, 7))
	if len(add) != 0 || !slices.Equal(remove, []uint32{3, 7}) {
		t.Fatalf("add %v remove %v, want [] and [3 7]", add, remove)
	}
}

func TestFlipRefreshPathsAreCardFilesOnly(t *testing.T) {
	if got := nvidiaDevicePath(12); got != "/dev/nvidia12" {
		t.Fatalf("path %q", got)
	}
}
