#!/bin/bash
# Run the release executable; default to this checkout's config, not its build path.
# Usage: start_system_report.sh [CONFIG] [--once|--dry-run|--print-config]
set -euo pipefail
TOP_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
BINARY="${TOP_DIR}/target/release/system_report"
if [ ! -x "${BINARY}" ]; then
    echo "no release binary found: run cargo build --release --locked in ${TOP_DIR}" >&2
    exit 1
fi
# Preserve caller cwd for explicit relative config and password-file paths.
# The CLI accepts the positional config before or after flags.
has_config=no
positional=no
for arg in "$@"; do
    if [ "${positional}" = yes ]; then
        has_config=yes
        break
    fi
    case "${arg}" in
        --) positional=yes ;;
        -*) ;;
        *) has_config=yes ;;
    esac
done
if [ "${has_config}" = no ]; then
    set -- "${TOP_DIR}/data/config.yaml" "$@"
fi
exec "${BINARY}" "$@"
