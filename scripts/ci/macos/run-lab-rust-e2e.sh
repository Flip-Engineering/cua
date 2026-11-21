#!/usr/bin/env bash
# Run the native matrix in the dedicated Flip lab's logged-in desktop.
set -euo pipefail

fail() { echo "::error::$*" >&2; exit 1; }

[[ "${GITHUB_REPOSITORY:-}" == Flip-Engineering/cua && "${RUNNER_ENVIRONMENT:-}" == self-hosted ]] \
  || fail "This lane requires the Flip Cua self-hosted Mac lab."
[[ "$(id -un)" == "$(stat -f '%Su' /dev/console)" ]] \
  && launchctl print "gui/$(id -u)" >/dev/null 2>&1 \
  || fail "Start the lab runner as the logged-in desktop user."
[[ "$(git rev-parse HEAD)" == "${CUA_E2E_SOURCE_SHA:?}" ]] \
  || fail "The checkout does not match the requested source SHA."

app=/Applications/CuaDriverLocal.app
driver="$app/Contents/MacOS/cua-driver-local"
if [[ -e "$app" ]]; then
  [[ -d "$app" && ! -L "$app" && "$(stat -f '%u' "$app")" == "$(id -u)" ]] \
    || fail "CuaDriverLocal.app must belong to the lab runner user."
fi

scratch="$(mktemp -d "$RUNNER_TEMP/cua-lab.XXXXXX")"
# Darwin limits Unix socket paths. Keep the socket outside a long work path.
socket_dir="$(mktemp -d /tmp/cua-lab-socket.XXXXXX)"
socket="$socket_dir/driver.sock"
launcher=
cleanup() {
  if [[ -S "$socket" ]]; then
    "$driver" --socket "$socket" stop >/dev/null 2>&1 || true
  fi
  if [[ -n "$launcher" ]]; then
    kill "$launcher" 2>/dev/null || true
    wait "$launcher" 2>/dev/null || true
  fi
  rmdir "$socket_dir" 2>/dev/null || true
}
trap cleanup EXIT

export CUA_DRIVER_LOCAL_HOME="$scratch/local"
export CUA_DRIVER_LOCAL_INSTALL_DIR="$scratch/bin"
export CARGO_TARGET_DIR="$scratch/target"
export CUA_DRIVER_SOURCE_SHA="$CUA_E2E_SOURCE_SHA"
bash libs/cua-driver/scripts/install-local.sh --release --require-stable-signing

open -W -n -g "$app" --args --socket "$socket" serve \
  --permission-mode unrestricted --dangerously-bypass-approvals &
launcher=$!
until [[ -S "$socket" ]]; do
  kill -0 "$launcher" 2>/dev/null || fail "The lab driver exited before opening its test socket."
  sleep 1
done

mkdir -p artifacts/cua-driver/macos-lab
"$driver" --socket "$socket" permissions status --json \
  | tee artifacts/cua-driver/macos-lab/permissions.json \
  | jq -e '.accessibility == true and .screen_recording == true' >/dev/null \
  || fail "Grant the signed CuaDriverLocal app Accessibility and Screen Recording in the lab user's System Settings."
export CUA_E2E_INSTALLED_DRIVER_BIN="$driver"
export CUA_E2E_MACOS_DAEMON_SOCKET="$socket"
export CUA_E2E_FRESH_FIXTURE_STATE=1
bash scripts/ci/macos/run-rust-e2e.sh
