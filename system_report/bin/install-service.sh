#!/bin/bash
# Render system_report.service for this checkout and install it.
#
#   sudo ./system_report/bin/install-service.sh [--user someuser] [--no-start]
#
# The unit runs as the invoking (non-root) user by default, from wherever this
# checkout lives. Builds must be performed beforehand as the ordinary user.

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
        --user) [ $# -ge 2 ] || { echo "--user requires a value" >&2; exit 2; }; RUN_AS="$2"; shift 2 ;;
        --no-start) DO_START="no"; shift ;;
        -h|--help)
            cat <<'USAGE'
Usage: sudo ./system_report/bin/install-service.sh [--user USER] [--no-start]
Install the prebuilt release executable as a systemd service.
--user USER  Run as this unprivileged user (default: invoking sudo user).
--no-start   Install and enable the unit without starting it.
USAGE
            exit 0 ;;

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
case "${TOP_DIR}" in
    *[!a-zA-Z0-9/_.-]*) echo "unsupported checkout path: use only letters, digits, /, _, ., -" >&2; exit 1 ;;
esac
if [ ! -x "${TOP_DIR}/target/release/system_report" ]; then
    echo "no release binary found: run cargo build --release --locked as ${RUN_AS} first" >&2
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
    if ! systemctl restart "${UNIT_NAME}"; then
        echo "failed to start ${UNIT_NAME}; recent journal follows" >&2
        journalctl --no-pager --unit="${UNIT_NAME}" --lines=30 >&2 || true
        exit 1
    fi
    sleep 2
    if ! systemctl --no-pager --full status "${UNIT_NAME}"; then
        echo "${UNIT_NAME} did not remain active after startup; recent journal follows" >&2
        journalctl --no-pager --unit="${UNIT_NAME}" --lines=30 >&2 || true
        exit 1
    fi
else
    echo "not started (--no-start). Start it with: systemctl start ${UNIT_NAME}"
fi
exit 0
