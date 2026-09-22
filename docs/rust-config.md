# Rust configuration slice

The Rust configuration library is used by the service executable. Build/test with
`cargo build --locked` and `cargo test --locked` (Rust 2024 edition).
Cargo.lock is committed.

## API

- `config::load(path: Option<&Path>, env: Option<&HashMap<String, String>>,
  hostname: Option<&str>) -> Result<Config, ConfigError>` reads a YAML file.
  The default is `data/config.yaml` in the **build checkout**, matching the old
  checkout-relative default. An installed binary must pass its config path
  explicitly; the checkout wrapper and systemd unit supply one.
- `Config::from_yaml(yaml, hostname, env)` has no ambient password lookup. Passing
  an empty environment disables ambient MQTT-password lookup. HOME expansion is
  separate: `~/` uses the supplied HOME first, then process HOME; inject HOME too
  for fully deterministic paths. `load(..., None, None)` reads the real process
  environment and hostname.
- `cfg["mqtt"]["port"].as_u64()` and similar read-only `serde_yaml::Value` access
  preserve the Python dictionary interface, recursive overlays and unknown keys.
  Known keys are validated at construction. No mutable indexing is exposed.
- Resolved `prefix`, `status_topic`, `json_topic`, `uptime_topic`, `hostname`,
  `password`, `warnings`, and `path` are public. `client_id()`, `meminfo_fields()`,
  `uptime_enabled()`, `meminfo_enabled()` and `meminfo_topic(field)` are helpers.
- `redacted()` returns a cloned config with inline passwords masked. Use it for
  diagnostics, never raw indexed values or `password`. Config deliberately does
  not implement Debug or Serialize. Parse/type errors do not echo YAML values.
- `log_warnings(|message| ...)` feeds the logging implementation.
- `consts` contains every Python constant; `consts::meminfo_topic_name` is the
  shared leaf-name helper for the collectors.

## Compatibility and stricter checks

All defaults, known topic names, the example YAML, and all scenarios from the
Python `test_config.py` are covered. Password precedence remains file > env >
inline. Relative password paths are relative to the process working directory,
not the YAML file. `~/` expands using HOME; `~otheruser` is not expanded.

The formatter supports the documented `{hostname}`, `{prefix}`, `{name}` and
escaped `{{`/`}}`, not arbitrary Python format conversions/specifiers. YAML uses
serde_yaml's YAML 1.2 scalar rules: use explicit `true`/`false`, not YAML 1.1
`yes`/`no` booleans. The example already uses these spellings.

Intentional hardening: malformed section types produce ConfigError rather than a
panic; non-finite timeouts are rejected; meminfo placeholders are checked at
startup; resolved topics are rechecked for wildcard/NUL injection. Empty explicit
environment maps do not fall back to the process environment (the Python
`env or os.environ` implementation did).

`serde_yaml` 0.9 is deprecated upstream but retained here per the port's decided
dependency mapping. MQTT uses `rumqttc`; readiness uses `sd-notify`.

`tests/fixtures/python_constants.json` is a frozen parity oracle generated on the
playground from the original `system_report/const.py` using `runpy.run_path`,
filtering uppercase names and serializing via `json.dump`. Rust tests require no
Python runtime. This oracle is retained after removal of the Python sources.
