# Upgrade an existing Python installation to Rust

> **DRAFT — not yet verified end to end on a live Linux host.** Keep this banner
> until a maintainer records the host/distribution, Rust version, commit, date,
> broker observations, and rollback result from actually running this recipe.

These steps assume the existing checkout, `env/` virtualenv, `data/config.yaml`,
and `system_report.service` layout. Run them in one Bash session as the ordinary
checkout owner with sudo available. Keep another terminal available for broker
observations. No step removes the Python environment or backups before validation.
Do not paste secrets or unredacted configuration into the PR.

## 1. Capture the existing installation

From the existing Python checkout:

```bash
CHECKOUT="$PWD"
PYTHON_REV="$(git rev-parse HEAD)"
SERVICE_USER="$(systemctl show system_report.service -p User --value)"
BACKUP="${CHECKOUT}-python-backup-$(date +%Y%m%d-%H%M%S)"
test -n "$SERVICE_USER" && test "$SERVICE_USER" != root
test -x env/bin/python
git diff --exit-code
git diff --cached --exit-code
sudo install -d -m 700 "$BACKUP"
sudo cp -a data "$BACKUP/data"
sudo cp -a /etc/systemd/system/system_report.service "$BACKUP/system_report.service"
if [ -d /etc/systemd/system/system_report.service.d ]; then
    sudo cp -a /etc/systemd/system/system_report.service.d "$BACKUP/"
fi
declare -p CHECKOUT PYTHON_REV SERVICE_USER BACKUP | sudo tee "$BACKUP/session-vars.sh" >/dev/null
sudo systemctl cat system_report.service | sudo tee "$BACKUP/unit-before.txt" >/dev/null
sudo systemctl show system_report.service | sudo tee "$BACKUP/state-before.txt" >/dev/null
sudo journalctl -u system_report.service -n 50 --no-pager | sudo tee "$BACKUP/journal-before.txt" >/dev/null
```

**Verify:** both Git diff commands succeed; save any work and stop otherwise.
Check `systemctl show system_report.service -p User -p WorkingDirectory -p ExecStart`
against this checkout. Record whether the service was enabled/running. Confirm
`sudo test -f "$BACKUP/data/config.yaml"` succeeds and the backup directory is
root-only. If a unit override changes its config, executable, or environment,
account for that override before proceeding. Back up referenced password/TLS
files outside `data/` with `sudo cp -a` into this protected backup as well.

## 2. Install the build prerequisites without replacing Python

Debian/Ubuntu example (use the equivalent packages on another distribution):

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev ca-certificates curl
if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/system-report-rustup.sh
    sh /tmp/system-report-rustup.sh -y --profile minimal --default-toolchain 1.88.0
fi
. "$HOME/.cargo/env"
rustup toolchain install 1.88.0 --profile minimal
rustc +1.88.0 --version
pkg-config --modversion openssl
```

**Verify:** Rust reports `1.88.0` and OpenSSL is found. `cargo` from an older
OS package is insufficient; use the explicit `+1.88.0` commands below. Build on
the target distribution to avoid newer-glibc/OpenSSL binary incompatibilities.
The old Python process and its virtualenv are still untouched.

## 3. Stop Python and select the reviewed Rust revision

While PR #1 is open, fetch its head explicitly. After it merges, fetch and select
the reviewed commit from `origin/main` instead of the PR ref.

```bash
sudo systemctl stop system_report.service
systemctl is-active system_report.service   # expected: inactive (nonzero exit)
cd "$CHECKOUT"
git fetch origin pull/1/head
RUST_REV="$(git rev-parse FETCH_HEAD)"
git show --stat --oneline "$RUST_REV"
git switch --detach "$RUST_REV"
test -f Cargo.toml
test -x env/bin/python
```

**Verify:** the service is inactive, the displayed revision is the one you intend
to deploy, and `env/bin/python` still exists. Do not delete or recreate `env/`.
If Git refuses the switch, stop and preserve the reported files; do not force it.

## 4. Carry forward configuration and secret access

Keep the existing `data/config.yaml`; do **not** replace it with the example.
Compare it locally with `data/config.yaml.example`. Change YAML booleans such as
`yes`/`no` to `true`/`false`. YAML anchors and `<<` merges are supported.
Password precedence remains file, environment, inline. Prefer absolute paths
for password and TLS files.

```bash
sudo -u "$SERVICE_USER" test -r "$CHECKOUT/data/config.yaml"
# If mqtt.password_file is configured, set this to that exact absolute path:
# PASSWORD_FILE=/absolute/path/to/password
# sudo chown "$SERVICE_USER" "$PASSWORD_FILE"
# sudo chmod 600 "$PASSWORD_FILE"
# sudo -u "$SERVICE_USER" test -r "$PASSWORD_FILE"
```

`data/system_report.env` is read by systemd as root and may remain root-owned,
mode 600. In contrast, the service process reads `mqtt.password_file`, certificate,
and key files. Their parent directories must be searchable by `SERVICE_USER`.
The installer only tightens the env file's permissions; it does not repair
arbitrary password-file ownership. A bad password-file path is a startup error.

If client TLS uses a legacy PKCS#1 key, convert it to a **new**, unencrypted
PKCS#8 file; preserve the original for rollback. Set these paths before running:

```bash
# OLD_KEY=/absolute/path/to/original-client.key
# NEW_KEY=/absolute/path/to/client-pkcs8.key
# test ! -e "$NEW_KEY"
# (umask 077; openssl pkcs8 -topk8 -nocrypt -in "$OLD_KEY" -out "$NEW_KEY")
# chmod 600 "$NEW_KEY"
# openssl pkey -in "$NEW_KEY" -check -noout
```

Run conversion as the key owner and ensure `SERVICE_USER` can read the result.
Update `mqtt.tls.keyfile` to the new path. A combined PEM certificate chain and
PKCS#8 key works in either order when `keyfile` is omitted. Do not disable TLS
verification to work around a key-format error.

**Verify:** readability checks succeed as the service user; required conversion
succeeds without displaying key contents. Confirm broker host, topics, client ID,
and TLS settings remain intentional. Persistent sessions retain exchanges across
network reconnects, but not a process replacement; plan for the old broker-side
session when using `clean_session: false`.

## 5. Build and check the Rust configuration

```bash
cd "$CHECKOUT"
cargo +1.88.0 build --release --locked
./target/release/system_report --version
sudo -u "$SERVICE_USER" ./target/release/system_report ./data/config.yaml --print-config
sudo -u "$SERVICE_USER" ./target/release/system_report ./data/config.yaml --dry-run
```

**Verify:** build succeeds, version prints, config validation succeeds, passwords
are redacted, and dry-run contains plausible Linux uptime/memory values and the
expected topics. These commands do not connect to the broker. They do not load
systemd's `EnvironmentFile`, so an env-only credential will only be checked in
the actual service. Do not source that root-owned file into a shell.

## 6. Install and verify the actual service and broker traffic

Start a subscriber in another terminal **before** starting the new service,
using the deployment's TLS/authentication settings without exposing passwords in
shell history. Example for an anonymous, non-TLS broker (adjust host/topics):

```bash
mosquitto_sub -h BROKER_HOST -v -R -t '/HOST/oper_state/#' -t '/HOST/oper_uptime_minutes'
```

`-R` ignores already-retained messages so stale `online` cannot pass this check.
Then install from the build checkout:

```bash
cd "$CHECKOUT"
sudo ./system_report/bin/install-service.sh --user "$SERVICE_USER"
systemctl is-active system_report.service
systemctl show system_report.service -p MainPID -p ExecStart -p User -p NRestarts
sudo journalctl -u system_report.service -n 100 --no-pager
```

**Verify:** unit is active, `ExecStart` names `target/release/system_report`, and
journal has no config, authentication, TLS, or DNS errors. Observe a **new**
`online` and at least one new uptime/memory report at the subscriber. With
`report_on_start: false`, wait the configured interval. `--once` returning zero
and systemd readiness alone do not prove delivery. Log counts represent publishes
completed within a timeout, not a total of broker deliveries.

Use the configured broker **hostname** under the installed unit to exercise DNS
inside its real sandbox. Leave `RestrictAddressFamilies` unchanged unless an
actual failure establishes that an additional family such as `AF_NETLINK` is
required on this host. Check `NRestarts` again after at least 180 seconds and
observe another reporting interval. Record results with the deployed revision.

## 7. Roll back if any verification fails

Before cleanup, the original virtualenv and Git revision remain available.
In a new shell, restore the variables by inspecting the protected
`$BACKUP/session-vars.sh` (or set them to the recorded values). Then:

```bash
sudo systemctl stop system_report.service
cd "$CHECKOUT"
git switch --detach "$PYTHON_REV"
sudo cp -a "$BACKUP/data/." "$CHECKOUT/data/"
sudo cp -a "$BACKUP/system_report.service" /etc/systemd/system/system_report.service
sudo systemctl daemon-reload
sudo systemctl restart system_report.service
systemctl is-active system_report.service
sudo journalctl -u system_report.service -n 100 --no-pager
```

Restore any external secrets you changed from their protected backups too.
Existing unit drop-ins were never removed; if you edited them, restore those
specific files. Reapply the recorded enabled/disabled state if needed.

**Verify:** the restored unit launches Python from the preserved environment,
stays active, and the subscriber sees fresh `online` and metric traffic. Record
this rollback result; do not claim rollback was tested without performing it.
To try Rust again, repeat from step 3 after correcting the failure.

## 8. Clean up only after successful verification and the rollback window

Keep the backup, original secrets, and Python virtualenv while you may still
roll back. Once the Rust deployment has been accepted and rollback is no longer
needed, remove only the old virtualenv from this checkout if desired:

```bash
cd "$CHECKOUT"
pwd                          # verify this is the intended checkout
# rm -rf -- "$CHECKOUT/env"  # enable only after ending the rollback window
```

**Verify:** the Rust service and broker traffic remain healthy. Do not run
`cargo clean` on this deployed checkout: the unit uses `target/release/system_report`.
Retire secret-bearing backups separately according to the host's backup policy.
Remove the draft banner only after recording an actual end-to-end host test,
including rollback, in the PR or this document.
