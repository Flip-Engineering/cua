#!/usr/bin/env bash
# Proves that a Space under Flip's runsc keeps its root filesystem writes across
# a stop and a start, with the runtime arguments Flip's updater registers: the
# tarball's RUNTIME_ARGS, the same file the updater reads. It fails when
# --overlay2=none is taken out of that file, and it also fails unless gVisor's
# default (no such flag) drops the write, so the test tells the two apart.
#
#   overlay-proof.sh <dir with runsc> <work dir>
#
# Starts its own dockerd (it needs root: the CI job runs it in a privileged
# container of its own), registers `flip` (Flip's arguments) and `gvisor-default`
# (gVisor's default overlay), and for each runtime: run a container, write a
# file outside any volume, stop, start, check the same container id, read the
# file, and look for it in `docker export`. Exit 0 only when `flip` keeps the
# file in both places; the default's result is printed as the control.
set -euo pipefail
runsc_dir="$(cd "${1:?dir with runsc}" && pwd)"
work="${2:?work dir}"
mkdir -p "$work/data" "$work/exec"
sock="unix://$work/docker.sock"
image="${PROOF_IMAGE:-debian:12-slim}"
proof=/root/flip-overlay-proof

args=$(sed 's/.*/"&"/' "$runsc_dir/RUNTIME_ARGS" | paste -sd, -)
cat >"$work/daemon.json" <<JSON
{
  "runtimes": {
    "flip": {"path": "$runsc_dir/runsc", "runtimeArgs": [$args]},
    "gvisor-default": {"path": "$runsc_dir/runsc", "runtimeArgs": ["--allow-suid"]}
  }
}
JSON
# In a container on a cgroup v2 host this shell's processes sit in the namespace's root
# cgroup, where no controller can be turned on for children ("device or resource
# busy"), so the engine could not give a sandbox its cgroup. As docker:dind does, move
# them into a child cgroup first and turn the controllers on for the root. Only inside a
# container (its own cgroup namespace): on a host this would move the host's processes.
if [ -f /.dockerenv ] && [ -f /sys/fs/cgroup/cgroup.controllers ] && [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  # A process the runner starts in the container meanwhile lands in the root again and
  # makes the write fail, so move what is there and write until the root is empty. A root
  # whose processes cannot move stops the proof with that reason.
  until sed -e "s/ / +/g" -e "s/^/+/" </sys/fs/cgroup/cgroup.controllers >/sys/fs/cgroup/cgroup.subtree_control 2>/dev/null; do
    moved=0
    while read -r pid; do
      if echo "$pid" >/sys/fs/cgroup/init/cgroup.procs 2>/dev/null; then moved=1; fi
    done </sys/fs/cgroup/cgroup.procs
    if [ "$moved" = 0 ]; then
      echo "overlay-proof: the container's root cgroup keeps processes that cannot move:" >&2
      cat /sys/fs/cgroup/cgroup.procs >&2
      exit 1
    fi
  done
fi
dockerd --config-file "$work/daemon.json" --data-root "$work/data" --exec-root "$work/exec" \
  --host "$sock" --pidfile "$work/dockerd.pid" >"$work/dockerd.log" 2>&1 &
d() { docker --host "$sock" "$@"; }
for _ in $(seq 60); do d info >/dev/null 2>&1 && break; sleep 1; done
d info >/dev/null
trap 'kill "$(cat "$work/dockerd.pid")" 2>/dev/null || true' EXIT
d pull -q "$image" >/dev/null

check() { # runtime -> prints "<kept after restart> <in export>"
  name="proof-$1"
  d rm -f "$name" >/dev/null 2>&1 || true
  id=$(d run -d --runtime "$1" --name "$name" "$image" sleep infinity)
  d exec "$name" sh -c "echo kept > $proof"
  d stop -t 5 "$name" >/dev/null
  d start "$name" >/dev/null
  [ "$(d inspect --format '{{.Id}}' "$name")" = "$id" ] || { echo "the container id changed" >&2; exit 1; }
  kept=no; d exec "$name" cat "$proof" 2>/dev/null | grep -qx kept && kept=yes
  # The listing goes to a file first: with pipefail, grep -q ending at the first match
  # kills tar with SIGPIPE and the pipeline fails, so a found file read as "no".
  d export "$name" | tar -t >"$work/export-$1.txt"
  exported=no; grep -qx "${proof#/}" "$work/export-$1.txt" && exported=yes
  d rm -f "$name" >/dev/null
  echo "$kept $exported"
}

read -r flip_kept flip_exported < <(check flip)
read -r def_kept def_exported < <(check gvisor-default)
echo "runsc $("$runsc_dir/runsc" --version | head -n 1)"
echo "flip ($(paste -sd' ' "$runsc_dir/RUNTIME_ARGS")): kept after stop and start: $flip_kept; in docker export: $flip_exported"
echo "gVisor's default overlay (control): kept after stop and start: $def_kept; in docker export: $def_exported"
[ "$flip_kept" = yes ] && [ "$flip_exported" = yes ] && [ "$def_kept" = no ]
