#!/bin/bash
# Start system_report from the checkout's virtualenv.
#
#   ./system_report/bin/start_system_report.sh [path/to/config.yaml] [--once|--dry-run]
#
# With no config argument, data/config.yaml next to this checkout is used.
#
# NOTE the exec below: systemd's Type=notify expects the READY=1 datagram from
# the unit's main process, so python has to *replace* this shell, not run as
# its child.

set -o errexit
set -o nounset

cd "$(dirname "$0")"
BIN_DIR="${PWD}"
PROG_DIR="${BIN_DIR%/*}"
TOP_DIR="${PROG_DIR%/*}"

if [ ! -e "${TOP_DIR}/env/bin/activate" ]; then
    echo "no virtualenv found: run ${BIN_DIR}/create-env.sh first" >&2
    exit 1
fi

source "${TOP_DIR}/env/bin/activate"
export PYTHONPATH="${TOP_DIR}${PYTHONPATH:+:${PYTHONPATH}}"

cd "${TOP_DIR}"
exec python -m system_report.main "$@"
