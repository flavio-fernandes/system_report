# system_report
#### Rust service that periodically reports Linux system state over MQTT

[![tests](https://github.com/flavio-fernandes/system_report/actions/workflows/tests.yml/badge.svg)](https://github.com/flavio-fernandes/system_report/actions/workflows/tests.yml)

## Goals

- Publish how much memory a machine has left, forever, without babysitting
- Survive a broker that is down at boot, down for a week, or moved to a new IP
- Keep every deployment-specific value in **one** configuration file
- Never grow: a memory reporter that leaks is worse than no reporter at all
- Keep secrets out of the git repository by construction, not by discipline
- Run as a single native executable, without a Python runtime

## Background

Chasing a slow memory leak on a headless device is miserable when the only
telemetry is "free memory is going down". `MemAvailable` alone says that
something is leaking, but never *which* bucket, and by the time you notice, the
history you needed is gone.

This project is the small, permanent half of that problem: a service that
publishes memory state to MQTT on a fixed cadence, so a long series already
exists the next time something starts eating RAM. It began as a generalization
of the `oper_state` reporting in
[bedclock](https://github.com/flavio-fernandes/bedclock), and it follows the
layout and conventions of
[mqtt2cmd](https://github.com/flavio-fernandes/mqtt2cmd).

Alongside `MemAvailable` it reports the kernel buckets that answer *which*:
`Slab`, `SUnreclaim`, `Shmem`, `PageTables`, `KernelStack` and friends. Plot
those over days and a slab leak, a growing tmpfs and a leaking user process look
completely different.

## What it publishes

With the defaults, a host named `myhost` publishes:

| Topic | Payload | Meaning |
|---|---|---|
| `/myhost/oper_state/status` | `online` / `offline` | retained; `offline` is the MQTT last will |
| `/myhost/oper_uptime_minutes` | `3262` | `/proc/uptime`, whole minutes |
| `/myhost/oper_state/mem_available_kb` | `408796` | `MemAvailable` from `/proc/meminfo`, kB |
| `/myhost/oper_state/slab_kb` | `89116` | one topic per configured field |
| ... | | `mem_total_kb`, `mem_free_kb`, `buffers_kb`, `cached_kb`, `shmem_kb`, `sreclaimable_kb`, `sunreclaim_kb`, `kernel_stack_kb`, `page_tables_kb`, `vmalloc_used_kb`, `swap_total_kb`, `swap_free_kb` |
| `/myhost/oper_state/json` | `{"ts":...}` | the whole report as one JSON document |

The two bedclock-compatible names (`oper_uptime_minutes` and
`oper_state/mem_available_kb`) are deliberate, so an existing subscriber or
forwarding rule keeps working.

Watching it live:

```bash
mosquitto_sub -h ${MQTT_BROKER} -v -t '/myhost/oper_state/#' -t '/myhost/oper_uptime_minutes'
```

```
/myhost/oper_state/status online
/myhost/oper_uptime_minutes 3262
/myhost/oper_state/mem_available_kb 408796
/myhost/oper_state/slab_kb 89116
/myhost/oper_state/json {"ts":"2026-09-18T01:50:46Z","version":"1.0.0","hostname":"myhost","uptime_s":195769,"uptime_minutes":3262,"mem_available_kb":408796,"slab_kb":89116}
```

Plain topics are easy to consume from anything (`mosquitto_sub`, Home
Assistant, a bridge to a cloud feed). The JSON topic is the one to record if you
plan to regress the series later.

## Requirements

- Linux with `/proc/meminfo` and `/proc/uptime`
- Rust 1.88 or newer (Rust 2024 edition) to build; no Python needed
- A C compiler, `pkg-config`, and OpenSSL development headers (on Debian/Ubuntu:
  `sudo apt-get install build-essential pkg-config libssl-dev`)
- An MQTT broker you can reach; systemd is optional for foreground use

MQTT uses `rumqttc`, YAML uses `serde_yaml`, and readiness uses `sd-notify`.
TLS uses the system OpenSSL libraries and CA trust store. Build on the target
Linux distribution (or a compatible one); a binary built against newer glibc or
OpenSSL is not guaranteed to run on an old distribution. Rust 1.88 is the
minimum supported Rust version (MSRV), checked on Linux in CI with the locked
dependencies. A [reported live upgrade](https://github.com/flavio-fernandes/system_report/pull/1#issuecomment-5787041561)
built and ran commit `60fa447` on Ubuntu 18.04.6 LTS with Rust 1.88.0 and system
OpenSSL 1.1.1, without special handling. This is evidence for that installation,
not a guarantee for every older Linux distribution.
Check `rustc --version`; distribution packages may be older. To install the
minimum toolchain as your ordinary user:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/system-report-rustup.sh
sh /tmp/system-report-rustup.sh --profile minimal --default-toolchain 1.88.0
. "$HOME/.cargo/env"
rustc --version
```

With an existing rustup installation, use `rustup toolchain install 1.88.0`
and `cargo +1.88.0 build --release --locked`.

## Installation

Build as your ordinary user, then write a config:

```bash
git clone https://github.com/flavio-fernandes/system_report.git
cd system_report
cargo build --release --locked
cp data/config.yaml.example data/config.yaml
$EDITOR data/config.yaml          # at minimum: mqtt.host
```

Check what it *would* publish, without touching the network:

```bash
./system_report/bin/start_system_report.sh --dry-run
```

Run against the real broker (Ctrl-C to stop). Set `knobs.log_to_console: true`
for foreground logs:

```bash
./system_report/bin/start_system_report.sh
# Or invoke the executable directly, always supplying the config path:
./target/release/system_report ./data/config.yaml
```

The wrapper supplies this checkout's `data/config.yaml` when no positional
config is given, even when called from another directory. Explicit relative
config paths are relative to your current directory. The binary's own omitted
config default is the **build-time checkout**; it is not relocatable. When
copying the binary elsewhere, always pass an explicit configuration path.
Password-file paths are also relative to the process working directory, not
the YAML file; prefer absolute paths for service deployments.

CLI options: `--once` publishes one report and exits, `--dry-run` prints messages
without connecting, `--print-config` prints redacted effective configuration,
and `--help` / `--version` print usage / version. `--once` waits a bounded time
for a broker and preserves exit status 0 when disconnected: it is not a delivery
health check.

Install the service after building and configuring:

```bash
sudo ./system_report/bin/install-service.sh
```

The installer renders the unit for this checkout and the invoking non-root
user, checks the release executable and config, enables and starts the service.
Useful flags: `--user someuser`, `--no-start`. Use a checkout path containing
only letters, digits, `/`, `_`, `-`, and `.` so it can be safely rendered into
the unit. The service executes the release binary directly with an explicit
config path. Keep the checkout in place; after updates, rebuild as your ordinary
user and run `sudo systemctl restart system_report.service`.

Migrating from Python: follow [the upgrade recipe](upgrade-from-python-recipe.md)
for backup, configuration checks, service and broker verification, and rollback.
The forward upgrade was exercised on a real host; rollback remains reviewed but
untested. Keep the Python virtualenv and backups through the rollback window.
Check for YAML 1.1 `yes` / `no` booleans and replace them with `true` / `false`
only if present; the recorded installation needed no configuration changes.

Watch the journal:

```bash
./system_report/bin/tail_log.sh
# Equivalent:
sudo journalctl --unit=system_report.service --lines=100 --follow --output=short-iso
```

Outside systemd, logging uses `/dev/log` when available, with stderr fallback;
`knobs.log_to_console: true` selects stderr explicitly.

### Installing by hand

Build first, choose an unprivileged user, then render the unit from the checkout
root (the same safe-path restriction applies):

```bash
sed -e "s|@USER@|$(id -un)|g" -e "s|@TOP_DIR@|${PWD}|g" \
    system_report/bin/system_report.service | sudo tee /etc/systemd/system/system_report.service
sudo systemctl daemon-reload
sudo systemctl enable --now system_report.service
```

## Configuration

Everything lives in `data/config.yaml`; every knob has a default, so the file
only carries what differs. `data/config.yaml.example` documents each one, and
`src/defaults.yaml` holds the values. Print what is actually in effect
(secrets redacted) with:

```bash
./system_report/bin/start_system_report.sh --print-config
```

### mqtt

| Key | Default | Meaning |
|---|---|---|
| `host` | `localhost` | broker address |
| `port` | `1883` | 1883 plain, usually 8883 for TLS |
| `client_id` | `system_report_<hostname>` | must be unique per broker |
| `keepalive_secs` | `60` | also how fast the broker declares this client dead |
| `username` | `null` | omit for an anonymous broker |
| `password_file` | `null` | file containing only the password (preferred) |
| `password_env` | `SYSTEM_REPORT_MQTT_PASSWORD` | environment variable holding it |
| `password` | `null` | inline; discouraged, warns at startup |
| `tls.enabled` | `false` | turn on TLS |
| `tls.ca_certs` | `null` | `null` means the system CA bundle |
| `tls.certfile` / `tls.keyfile` | `null` | PEM client certificate chain and unencrypted PKCS#8 key; omitted keyfile uses combined certfile |
| `tls.insecure` | `false` | disables hostname verification only; CA validation remains enabled. Testing only |
| `qos` | `0` | QoS for metric publishes |
| `retain` | `false` | retain metric values (status is retained separately) |
| `clean_session` | `true` | |
| `reconnect_min_delay_secs` | `1` | backoff floor |
| `reconnect_max_delay_secs` | `120` | backoff ceiling |
| `recreate_client_after_secs` | `900` | rebuild after this long offline for clean sessions; `0` disables; ignored for persistent sessions |
| `publish_timeout_secs` | `10.0` | wait for QoS 0 socket write or QoS 1/2 acknowledgement; timeout is not cancellation |
| `max_queued_messages` | `100` | maximum outstanding QoS 1/2 publications; `0` removes this cap; transport flow control still applies |

### topics

| Key | Default | Meaning |
|---|---|---|
| `prefix` | `/{hostname}` | `{hostname}` is expanded at startup |
| `status` | `{prefix}/oper_state/status` | online/offline topic |
| `payload_online` / `payload_offline` | `online` / `offline` | |
| `status_retain` | `true` | keep the last status for new subscribers |
| `json` | `{prefix}/oper_state/json` | JSON snapshot topic |

### report

| Key | Default | Meaning |
|---|---|---|
| `interval_secs` | `600` | seconds between reports |
| `report_on_start` | `true` | `false` waits one full interval before the first report |
| `publish_plain` | `true` | one plain number per topic |
| `publish_json` | `true` | the JSON snapshot |

### metrics

| Key | Default | Meaning |
|---|---|---|
| `uptime_minutes.enabled` | `true` | |
| `uptime_minutes.topic` | `{prefix}/oper_uptime_minutes` | |
| `meminfo.enabled` | `true` | |
| `meminfo.topic` | `{prefix}/oper_state/{name}` | `{name}` is the per-field leaf name |
| `meminfo.fields` | see `config.yaml.example` | any `/proc/meminfo` field name |

Field names are mapped to topic leaves in `src/consts.rs`
(`MemAvailable` → `mem_available_kb`); anything unmapped falls back to
snake_case. A field your kernel does not have is logged once per report and
skipped, not fatal.

### knobs

`log_to_console` and `log_level_debug`, both `false`. Under systemd everything
already goes to the journal; these are for foreground debugging.

## Resilience

The point of this service is to still be publishing months from now.

- **The broker does not have to exist.** Connecting is asynchronous, so a
  broker that is down at boot (or for the next week) never blocks or crashes
  the reporter. Reconnects back off between `reconnect_min_delay_secs` and
  `reconnect_max_delay_secs`.
- **Belt and braces.** If the connection has been down for
  `recreate_client_after_secs`, the MQTT client object is thrown away and
  rebuilt, which recovers from a wedged socket or a dead network thread that a
  plain reconnect loop would sit in forever. With `clean_session: false`, this
  recreation is disabled to preserve unfinished broker exchanges and packet IDs.
  Persistent sessions survive network reconnects within this process; protocol
  state is not saved to disk across process restarts.
- **Reports are dropped, not buffered.** While the broker is unreachable, the
  slot is skipped and logged; nothing accumulates. Memory stays flat through an
  outage, which is the whole point of a leak reporter. The Python implementation
  had a flat-RSS outage measurement; the Rust port does not yet have an equivalent
  long-running memory benchmark.
- **No catch-up bursts.** The schedule is monotonic and always counts from "now", so
  a machine that was suspended for a day publishes one report when it wakes, not
  a hundred.
- **The last will covers hard failures.** `kill -9`, a kernel panic or a pulled
  cable all end with the broker publishing `offline` on the status topic.
- **A hung loop is caught by systemd.** The unit is `Type=notify` with
  `WatchdogSec=180`; the loop pets the watchdog on every tick. Note that being
  disconnected from the broker does *not* stop the petting: an unreachable
  broker is an expected state, not a reason to restart.
- **Collectors fail independently.** An unreadable `/proc/uptime` does not stop
  the `/proc/meminfo` values from being published, and vice versa.

## Security and privacy

- **Secrets never enter the repository.** `.gitignore` ignores all of `data/`
  except the `*.example` files, so your `config.yaml`, env file and password
  file cannot be committed by accident, including any new file dropped there
  later. `*.env`, `*.secret`, `*.pem`, `*.key` are ignored anywhere in the tree.
- **A second lock, if you want it:** `./system_report/bin/install-git-hooks.sh`
  installs a pre-commit hook that rejects those paths even with `git add -f`,
  and refuses a commit containing what looks like an inline password.
- **Prefer a file or the environment over an inline password.** The service
  reads `mqtt.password_file` first, then `mqtt.password_env`, then `password`.
  Using the inline value logs a warning, as does a secret file that is readable
  beyond its owner.
- **The systemd unit reads the env file as root** before dropping privileges,
  so `data/system_report.env` can stay root-owned with `chmod 600`. In contrast,
  the **service user** reads `mqtt.password_file`: that file must be owned/readable
  by that user (normally mode 600), with searchable parent directories. The
  installer does not change arbitrary password-file ownership. Use absolute
  paths and check `sudo -u SERVICE_USER test -r /path/to/password`.
- **Nothing is logged that should not be.** `--print-config` and the startup
  log redact the password.
- **TLS is off by default because most home brokers are.** If you enable
  authentication, enable TLS too: otherwise the password crosses the network in
  the clear, and the service says so at startup.
- **It publishes only aggregate numbers.** No process names, no command lines
  (which routinely contain credentials), no user data. The one identifier that
  leaves the machine is the hostname in the topic prefix; set `topics.prefix` to
  a fixed string if even that is too much.
- **It is publish-only.** The service subscribes to nothing, so no MQTT message
  can make it do anything.
- **It never writes to disk** and needs no privileges beyond reading `/proc`.
  The unit runs unprivileged with `NoNewPrivileges`, `ProtectSystem=full`,
  `ProtectHome=read-only`, `PrivateTmp`, `PrivateDevices` and a restricted set
  of address families.

## Uninstall

```bash
sudo systemctl disable --now system_report.service
sudo rm -f /etc/systemd/system/system_report.service
sudo systemctl daemon-reload
```

After uninstalling, `cargo clean` removes this checkout's Rust build artifacts.
The old `env/` virtualenv can be removed after the rollback window has closed.
Keep `data/`, external secrets/certificates, and backups until deliberately retired;
removing the unit does not delete them.

The retained `offline` status stays on the broker until someone clears it:

```bash
mosquitto_pub -h ${MQTT_BROKER} -r -n -t '/myhost/oper_state/status'
```

## Development

```bash
rustup component add rustfmt clippy
cargo build --release --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
bash tests/scripts.sh
```

### Continuous integration

[.github/workflows/tests.yml](.github/workflows/tests.yml) retains pushes and
pull requests to `main`, plus manual dispatch and read-only repository access.

| Job | What it covers |
|---|---|
| Rust 1.88 (minimum) | locked release build and all tests on Ubuntu 24.04 |
| Rust stable | locked release build, all tests, warnings-denied clippy, rustfmt |
| Rust beta (informational) | the same checks against the next Rust release; allowed to fail |
| hygiene | shell syntax, installer/wrapper smoke tests, tracked-data and inline-secret guards |

Rust checks replace the Python interpreter and paho callback-API matrix;
there is no Python test dependency. To reproduce
the stable job in a Linux container:

```bash
docker run --rm -v "${PWD}:/work" -w /work rust:1 sh -ec '
  rustup component add rustfmt clippy
  cargo build --release --locked
  cargo test --locked
  cargo clippy --locked --all-targets -- -D warnings
  cargo fmt --all -- --check
'
```

Parser and scheduler tests use fixtures and fake clocks. MQTT tests use scripted
loopback peers (no external broker); executable tests read live `/proc`, exercise
signals and Unix notification sockets. Full TLS handshakes and broker-delivered
last wills are not integration-tested. `tests/fixtures/python_constants.json`
is retained as a frozen compatibility oracle, not executable Python.
`--dry-run` prints a real report without network access.

Implementation notes: [configuration](docs/rust-config.md),
[collectors/logging](docs/rust-collectors-logging.md),
[MQTT](docs/rust-mqtt.md), [systemd notifications](docs/rust-sdnotify.md).

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `no release binary found` | run `cargo build --release --locked` in this checkout |
| OpenSSL build error | install the compiler, `pkg-config` and OpenSSL development headers |
| Startup cannot read config after moving binary | pass an explicit config path |
| Startup fails with a config validation error | check the named key and use YAML `true` / `false` booleans |
| Nothing arrives | check broker host/port, firewall, credentials and TLS settings; enable console logs |
| Reports dropped while disconnected | expected during an outage; reconnects happen automatically |
| Values look stale | `report.interval_secs` defaults to 600 |
| Service restarts every few minutes | inspect the journal and watchdog failures |

## Alternatives

Worth knowing before adopting this, because if one of them fits, it is less code
to own:

- **[Telegraf](https://docs.influxdata.com/telegraf/v1/output-plugins/mqtt/)** is
  the closest off-the-shelf equivalent: `inputs.mem` plus `outputs.mqtt` with
  `layout = "field"` publishes one plain value per topic, it reconnects, and it
  supports secret stores. Costs: a Go daemon an order of magnitude larger in
  memory than this, and TOML config. On a box with a gigabyte of RAM, that is a
  real trade-off; on a server, Telegraf is probably the better answer.
- **[collectd](https://github.com/collectd/collectd/wiki/Plugin-MQTT)** is tiny
  and packaged everywhere, but its MQTT plugin publishes
  `<unixtime>:<value>` payloads under its own topic scheme, and it has no
  `MemAvailable` type.
- **[OpenTelemetry Collector](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/main/receiver/hostmetricsreceiver)**
  has an excellent host metrics receiver, but no MQTT exporter: it is built for
  OTLP/Prometheus backends, and it is heavy for a small device.
- **[linux2mqtt](https://github.com/miaucl/linux2mqtt)**, `system_sensors`,
  `lnxlink` and similar publish far more (CPU, disks, Home Assistant discovery)
  and are a better fit if you want a dashboard rather than a long, boring
  memory series. They need Python 3.7+ (often 3.10+), so they do not run on
  older boxes.

## License

MIT, see [LICENSE](LICENSE). Contributions and issues welcome.
