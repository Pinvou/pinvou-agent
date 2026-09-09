#!/bin/sh
# Cargo calls this after every build, including fresh/no-op builds. exec keeps
# application arguments, exit status and Tauri's process lifecycle intact.
set -eu
signing_node="$1"
shift
"$signing_node" "$(dirname "$0")/macos-dev-signing.js" sign "$1"
exec "$@"
