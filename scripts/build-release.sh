#!/usr/bin/env bash
set +xv
set -euo pipefail
if (( $# > 1 )); then
    echo 'Usage: scripts/build-release.sh [oauth-env-file]' >&2
    exit 2
fi
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
env_file=${1:-"$repo/.env.oauth"}
if [[ ! -f "$env_file" ]]; then
    echo 'OAuth env file not found; pass a file or create .env.oauth in the repo.' >&2
    exit 1
fi
set -a
if ! source "$env_file" >/dev/null 2>&1; then
    echo 'Could not load OAuth env file.' >&2
    exit 1
fi
set +a
cd -- "$repo"
exec cargo build --release -p lagom
