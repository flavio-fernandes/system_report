#!/bin/bash
# Create the ./env virtualenv with everything system_report needs.
#
#   ./system_report/bin/create-env.sh            # runtime dependencies
#   ./system_report/bin/create-env.sh --with-tests   # ... plus pytest/flake8
#
# Safe to re-run: an existing env is reused and its packages refreshed.

set -o errexit
set -o xtrace
# NOTE: no `set -o nounset` here. The virtualenv's activate script ships a
# deactivate() that reads $1 unguarded, which would abort this script.

cd "$(dirname "$0")"
BIN_DIR="${PWD}"
PROG_DIR="${BIN_DIR%/*}"
TOP_DIR="${PROG_DIR%/*}"

WITH_TESTS="no"
[ "${1:-}" = "--with-tests" ] && WITH_TESTS="yes"

cd "${TOP_DIR}"
if [ ! -e ./env ]; then
    python3 -m venv --copies env
fi
source ./env/bin/activate

# pip honors Requires-Python, so this lands on the newest pip that still
# supports the interpreter in use (21.3.1 on Python 3.6, latest elsewhere).
python -m pip install --upgrade pip || true
python -m pip install -r ./requirements.txt
if [ "${WITH_TESTS}" = "yes" ]; then
    python -m pip install -r ./test_requirements.txt
fi

python -m pip list
deactivate

set +o xtrace
echo
echo "env ready. Next:"
echo "  cp ${TOP_DIR}/data/config.yaml.example ${TOP_DIR}/data/config.yaml"
echo "  \$EDITOR ${TOP_DIR}/data/config.yaml"
echo "  ${TOP_DIR}/system_report/bin/start_system_report.sh --dry-run"
exit 0
