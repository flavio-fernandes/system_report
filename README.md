# system_report
#### Python based service that periodically reports Linux system state over MQTT

[![tests](https://github.com/flavio-fernandes/system_report/actions/workflows/tests.yml/badge.svg)](https://github.com/flavio-fernandes/system_report/actions/workflows/tests.yml)

## Goals

- Publish how much memory a machine has left, forever, without babysitting
- Survive a broker that is down at boot, down for a week, or moved to a new IP
- Keep every deployment-specific value in **one** configuration file
- Never grow: a memory reporter that leaks is worse than no reporter at all
- Keep secrets out of the git repository by construction, not by discipline
- Run on an old, small box: Python 3.6 and ~20 MB of RSS are enough

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

- Linux with `/proc/meminfo` and `/proc/uptime` (that is, Linux)
- Python 3.6 or newer, with `python3-venv`
- An MQTT broker you can reach

Developed and run on Ubuntu 18.04 with Python 3.6.9 and systemd 237; there is
nothing in it that a newer distribution would mind. Dependencies are
[paho-mqtt](https://pypi.org/project/paho-mqtt/) and
[PyYAML](https://pypi.org/project/PyYAML/); both callback APIs of paho (1.x and
2.x) are supported.

## Installation

Clone, build the virtualenv, write a config:

```bash
git clone https://github.com/flavio-fernandes/system_report.git
cd system_report
./system_report/bin/create-env.sh
cp data/config.yaml.example data/config.yaml
$EDITOR data/config.yaml          # at minimum: mqtt.host
```

Check what it *would* publish, without touching the network:

```bash
./system_report/bin/start_system_report.sh --dry-run
```

Run it in the foreground against the real broker (Ctrl-C to stop). Set
`knobs.log_to_console: true` in the config first, so you can see what happens:

```bash
./system_report/bin/start_system_report.sh
```

Then install it as a service. The script renders the unit template for this
checkout and this user, so there is nothing to edit by hand:

```bash
sudo ./system_report/bin/install-service.sh
```

It refuses to run the service as root, insists on an existing virtualenv and
config, enables the unit, and starts it. Useful flags: `--user someuser`,
`--no-start`.

Watch the log:

```bash
./system_report/bin/tail_log.sh
```

Which is just:

```bash
sudo journalctl --unit=system_report.service --lines=100 --follow --output=short-iso
```

### Installing by hand

If you would rather not use the script, copy the template and replace the two
placeholders yourself:

```bash
sed -e "s|@USER@|$(id -un)|g" -e "s|@TOP_DIR@|${PWD}|g" \
    system_report/bin/system_report.service | sudo tee /etc/systemd/system/system_report.service
sudo systemctl daemon-reload
sudo systemctl enable --now system_report.service
```

## Configuration

Everything lives in `data/config.yaml`; every knob has a default, so the file
only carries what differs. `data/config.yaml.example` documents each one, and
`system_report/const.py` holds the values. Print what is actually in effect
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
| `tls.certfile` / `tls.keyfile` | `null` | client certificate, if required |
| `tls.insecure` | `false` | disables certificate checks. Testing only |
| `qos` | `0` | QoS for metric publishes |
| `retain` | `false` | retain metric values (status is retained separately) |
| `clean_session` | `true` | |
| `reconnect_min_delay_secs` | `1` | backoff floor |
| `reconnect_max_delay_secs` | `120` | backoff ceiling |
| `recreate_client_after_secs` | `900` | rebuild the client after this long offline; `0` disables |
| `publish_timeout_secs` | `10.0` | how long to wait for a QoS>0 acknowledgement |
| `max_queued_messages` | `100` | cap on the library's internal queue |

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

Field names are mapped to topic leaves in `system_report/const.py`
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
  plain reconnect loop would sit in forever.
- **Reports are dropped, not buffered.** While the broker is unreachable, the
  slot is skipped and logged; nothing accumulates. Memory stays flat through an
  outage, which is the whole point of a leak reporter. Measured: RSS was
  unchanged across a 75-second outage and recovery.
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
  so `data/system_report.env` can stay `chmod 600`.
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

The retained `offline` status stays on the broker until someone clears it:

```bash
mosquitto_pub -h ${MQTT_BROKER} -r -n -t '/myhost/oper_state/status'
```

## Development

```bash
./system_report/bin/create-env.sh --with-tests
PYTHONPATH=. ./env/bin/python -m pytest system_report/tests/unit
./env/bin/python -m flake8 system_report
```

Or with tox, if you have it: `tox`.

### Continuous integration

[.github/workflows/tests.yml](.github/workflows/tests.yml) runs on every push and
pull request to `main`:

| Job | What it covers |
|---|---|
| Python 3.6 | pytest and flake8 inside the `python:3.6-slim` container — the interpreter this is actually deployed on, and the one GitHub's runners no longer ship |
| Python 3.9 / 3.11 / 3.13 / latest stable | the same tests against paho-mqtt 2.x, which proves the callback-API shim rather than just claiming it. The last entry is `3.x`, so it follows each new release by itself |
| Python pre-release | the next Python, early. Allowed to fail: a beta breaking a dependency should not turn the repo red |
| hygiene | `bash -n` on every script, plus the promise this project makes about secrets: only `*.example` files under `data/`, and no inline password anywhere in the tree |

To reproduce the 3.6 job locally, if you have docker:

```bash
docker run --rm -v "${PWD}:/src" -w /src python:3.6-slim sh -exc 'pip install -r requirements.txt -r test_requirements.txt && python -m pytest system_report/tests/unit -q && python -m flake8 system_report'
```

The tests are pure and fast (no broker, no sleeping, no `/proc`): the parsers
take text, the scheduler takes a fake clock, and the publisher takes a fake MQTT
client. `--dry-run` prints a real report without touching the network, which is
the quickest way to see the effect of a config change.

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| `no virtualenv found` | run `create-env.sh` |
| Startup fails with `config key '...'` | the message names the key and the expected value |
| Nothing arrives, log says `connecting to mqtt broker` only | wrong host/port, firewall, or broker requires auth/TLS |
| `broker ... refused the connection: 5` | bad credentials |
| `broker not connected (...): dropped N message(s)` | expected during an outage; it recovers by itself |
| Values look stale | that is the cadence: `report.interval_secs` defaults to 600 |
| Service restarts every few minutes | look for `unexpected failure in the report loop` in the journal |

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
