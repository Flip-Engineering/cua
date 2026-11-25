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

// Flip's runsc build adds this file to runsc/cmd/nvproxy.

package nvproxy

import (
	"context"
	"encoding/json"
	"os"

	"github.com/google/subcommands"
	"gvisor.dev/gvisor/runsc/client"
	"gvisor.dev/gvisor/runsc/cmd/util"
	"gvisor.dev/gvisor/runsc/config"
	"gvisor.dev/gvisor/runsc/container"
	"gvisor.dev/gvisor/runsc/flag"
)

// refresh implements subcommands.Command for "nvproxy refresh".
type refresh struct{}

// Name implements subcommands.Command.Name.
func (*refresh) Name() string {
	return "refresh"
}

// Synopsis implements subcommands.Command.Synopsis.
func (*refresh) Synopsis() string {
	return "make a running container's GPU device files equal to the host's cards"
}

// Usage implements subcommands.Command.Usage.
func (*refresh) Usage() string {
	return "refresh <container id> - add the device files of cards the host gained and remove those of cards that left; prints {\"added\":[...],\"removed\":[...]}.\n"
}

// SetFlags implements subcommands.Command.SetFlags.
func (*refresh) SetFlags(*flag.FlagSet) {}

// Execute implements subcommands.Command.Execute.
func (*refresh) Execute(ctx context.Context, f *flag.FlagSet, args ...any) subcommands.ExitStatus {
	if f.NArg() != 1 {
		f.Usage()
		return subcommands.ExitUsageError
	}
	conf := args[0].(*config.Config)
	cont, err := client.New(conf).Load(ctx, f.Arg(0), container.LoadOpts{})
	if err != nil {
		util.Fatalf("loading container %q: %v", f.Arg(0), err)
	}
	reply, err := cont.Sandbox.RefreshNvproxyDevices(cont.ID)
	if err != nil {
		util.Fatalf("%v", err)
	}
	if err := json.NewEncoder(os.Stdout).Encode(reply); err != nil {
		util.Fatalf("%v", err)
	}
	return subcommands.ExitSuccess
}
