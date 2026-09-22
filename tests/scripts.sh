#!/bin/bash
# Isolated smoke tests: no root, installed service, broker or Rust build required.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/checkout/system_report/bin" "$TMP/checkout/target/release" "$TMP/checkout/data" "$TMP/mock"
cp "$ROOT"/system_report/bin/* "$TMP/checkout/system_report/bin/"
START="$TMP/checkout/system_report/bin/start_system_report.sh"
INSTALL="$TMP/checkout/system_report/bin/install-service.sh"
if bash "$START" --dry-run >"$TMP/out" 2>&1; then
    echo 'missing binary was accepted' >&2; exit 1
fi
grep -q 'cargo build --release --locked' "$TMP/out"
cat > "$TMP/checkout/target/release/system_report" <<'STUB'
#!/bin/bash
printf '%s\n' "$PWD" "$@"
STUB
chmod +x "$TMP/checkout/target/release/system_report"
cd "$TMP"
bash "$START" --dry-run > actual
printf '%s\n' "$TMP" "$TMP/checkout/data/config.yaml" --dry-run > expected
diff -u expected actual
bash "$START" --dry-run relative.yaml > actual
printf '%s\n' "$TMP" --dry-run relative.yaml > expected
diff -u expected actual
bash "$START" -- --odd-name.yaml > actual
printf '%s\n' "$TMP" -- --odd-name.yaml > expected
diff -u expected actual
# Redirect only the copied installer's system unit destination.
sed -i "s|UNIT_DIR=\"/etc/systemd/system\"|UNIT_DIR=\"$TMP/units\"|" "$INSTALL"
mkdir "$TMP/units"
cat > "$TMP/mock/id" <<'STUB'
#!/bin/bash
case "$*" in
    '-u') echo 0 ;;
    '-un') echo root ;;
    '-u tester') echo 1000 ;;
    *) exit 1 ;;
esac
STUB
cat > "$TMP/mock/systemctl" <<'STUB'
#!/bin/bash
printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
STUB
chmod +x "$TMP/mock/"*
export PATH="$TMP/mock:$PATH" SYSTEMCTL_LOG="$TMP/systemctl.log"
expect_failure() {
    if bash "$INSTALL" "$@" > "$TMP/out" 2>&1; then
        echo "installer unexpectedly succeeded: $*" >&2; exit 1
    fi
}
expect_failure --user
expect_failure --user root
expect_failure --user nonexistent
expect_failure --user tester --no-start
grep -q 'config.yaml' "$TMP/out"
touch "$TMP/checkout/data/config.yaml" "$TMP/checkout/data/system_report.env"
bash "$INSTALL" --user tester --no-start
grep -Fx 'User=tester' "$TMP/units/system_report.service"
grep -Fx "ExecStart=$TMP/checkout/target/release/system_report $TMP/checkout/data/config.yaml" "$TMP/units/system_report.service"
! grep -q '@' "$TMP/units/system_report.service"
[ "$(stat -c %a "$TMP/checkout/data/system_report.env")" = 600 ]
printf '%s\n' daemon-reload 'enable system_report.service' > expected
diff -u expected "$SYSTEMCTL_LOG"
rm "$TMP/checkout/target/release/system_report"
expect_failure --user tester --no-start
grep -q 'cargo build --release --locked' "$TMP/out"
echo 'shell smoke tests: PASS'
