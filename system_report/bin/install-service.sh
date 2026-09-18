#!/bin/bash
# Render system_report.service for this checkout and install it.
#
#   sudo ./system_report/bin/install-service.sh [--user someuser] [--no-start]
#
# The unit runs as the invoking (non-root) user by default, from wherever this
# checkout lives. Nothing is written anywhere else.

set -o errexit
set -o nounset

cd "$(dirname "$0")"
BIN_DIR="${PWD}"
PROG_DIR="${BIN_DIR%/*}"
TOP_DIR="${PROG_DIR%/*}"

UNIT_NAME="system_report.service"
UNIT_DIR="/etc/systemd/system"
RUN_AS="${SUDO_USER:-$(id -un)}"
DO_START="yes"

while [ $# -gt 0 ]; do
    case "$1" in
        --user) RUN_AS="$2"; shift 2 ;;
        --no-start) DO_START="no"; shift ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ "$(id -u)" != "0" ]; then
    echo "this needs root to write ${UNIT_DIR}/${UNIT_NAME}; re-run with sudo" >&2
    exit 1
fi
if [ "${RUN_AS}" = "root" ]; then
    echo "refusing to run the service as root: pass --user <unprivileged-user>" >&2
    exit 1
fi
if ! id -u "${RUN_AS}" >/dev/null 2>&1; then
    echo "user does not exist: ${RUN_AS}" >&2
    exit 1
fi
if [ ! -e "${TOP_DIR}/env/bin/activate" ]; then
    echo "no virtualenv in ${TOP_DIR}/env: run create-env.sh as ${RUN_AS} first" >&2
    exit 1
fi
if [ ! -e "${TOP_DIR}/data/config.yaml" ]; then
    echo "no ${TOP_DIR}/data/config.yaml: copy data/config.yaml.example and edit it first" >&2
    exit 1
fi

sed -e "s|@USER@|${RUN_AS}|g" \
    -e "s|@TOP_DIR@|${TOP_DIR}|g" \
    "${BIN_DIR}/${UNIT_NAME}" > "${UNIT_DIR}/${UNIT_NAME}"
chmod 644 "${UNIT_DIR}/${UNIT_NAME}"
echo "wrote ${UNIT_DIR}/${UNIT_NAME} (User=${RUN_AS}, checkout ${TOP_DIR})"

# If a secrets file is present, make sure it is not readable by anyone else.
ENV_FILE="${TOP_DIR}/data/system_report.env"
if [ -e "${ENV_FILE}" ]; then
    chmod 600 "${ENV_FILE}"
    echo "tightened permissions on ${ENV_FILE} (chmod 600)"
fi

systemctl daemon-reload
systemctl enable "${UNIT_NAME}"
if [ "${DO_START}" = "yes" ]; then
    systemctl restart "${UNIT_NAME}"
    sleep 2
    systemctl --no-pager --full status "${UNIT_NAME}" || true
else
    echo "not started (--no-start). Start it with: systemctl start ${UNIT_NAME}"
fi
exit 0
