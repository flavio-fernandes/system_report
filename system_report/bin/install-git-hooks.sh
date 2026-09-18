#!/bin/bash
# Install a pre-commit hook that refuses to commit secrets.
#
#   ./system_report/bin/install-git-hooks.sh
#
# .gitignore already keeps data/ (except *.example), *.env and *.secret out of
# the index. This hook is the second lock: it also catches `git add -f` and a
# non-empty inline password in a file that is otherwise fine to commit.

set -o errexit
set -o nounset

cd "$(dirname "$0")"
TOP_DIR="$(cd ../.. && pwd)"
HOOK_DIR="$(git -C "${TOP_DIR}" rev-parse --git-path hooks)"
case "${HOOK_DIR}" in
    /*) ;;
    *) HOOK_DIR="${TOP_DIR}/${HOOK_DIR}" ;;
esac
mkdir -p "${HOOK_DIR}"
HOOK="${HOOK_DIR}/pre-commit"

if [ -e "${HOOK}" ] && ! grep -q 'system_report secret guard' "${HOOK}" 2>/dev/null; then
    echo "refusing to overwrite an existing hook: ${HOOK}" >&2
    exit 1
fi

cat > "${HOOK}" <<'HOOK_EOF'
#!/bin/bash
# system_report secret guard (installed by system_report/bin/install-git-hooks.sh)
set -o nounset

fail() { echo "pre-commit: $*" >&2; exit 1; }

staged="$(git diff --cached --name-only --diff-filter=ACM)"
[ -z "${staged}" ] && exit 0

while IFS= read -r file; do
    case "${file}" in
        *.example) continue ;;
        data/*) fail "${file} is host-specific/secret; it must not be committed" ;;
        *.env|*.secret|*.pem|*.key|*mqtt_password*)
            fail "${file} looks like a secret; it must not be committed" ;;
    esac
done <<< "${staged}"

# An inline password with an actual value (password: null / "" are fine).
while IFS= read -r file; do
    case "${file}" in *.example) continue ;; esac
    if git show ":${file}" 2>/dev/null \
        | grep -nEi '^[[:space:]]*(password|passwd|token|api[_-]?key)[[:space:]]*:[[:space:]]*[^[:space:]#]' \
        | grep -vEi ':[[:space:]]*(null|~|""|'"''"')[[:space:]]*$' >/dev/null; then
        fail "${file} contains what looks like an inline secret"
    fi
done <<< "${staged}"

exit 0
HOOK_EOF

chmod 755 "${HOOK}"
echo "installed ${HOOK}"
exit 0
