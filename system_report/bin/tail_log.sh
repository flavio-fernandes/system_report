#!/usr/bin/env bash
# Follow the service log in the systemd journal.
#
#   ./system_report/bin/tail_log.sh
#   SYSTEM_REPORT_LOG_LINES=500 ./system_report/bin/tail_log.sh

set -o errexit
set -o nounset
set -o pipefail

UNIT="${SYSTEM_REPORT_SYSTEMD_UNIT:-system_report.service}"
LOG_LINES="${SYSTEM_REPORT_LOG_LINES:-100}"

exec sudo journalctl \
    --unit="${UNIT}" \
    --lines="${LOG_LINES}" \
    --follow \
    --output=short-iso \
    "$@"
