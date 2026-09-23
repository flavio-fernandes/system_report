# Rust collectors and logging

These library modules are used by the Rust service executable. Existing config
and constants parity tests remain intact.

## Collectors

`collectors::collect(&CollectOptions::default())` reads the real Linux
`/proc/uptime` and `/proc/meminfo`. `collect_with_reader` accepts an
`FnMut(&str) -> io::Result<String>` for tests or an alternate proc mount.
Options select fields, enable collectors independently, and optionally inject
hostname and UTC time. Callbacks must not panic.

`Snapshot` contains a `DateTime<Utc>`, optional hostname and uptime values,
ordered `(String, i64)` memory pairs, and independent error strings. Memory
values are raw kernel numbers (normally kB; unitless fields stay unitless).
Explicit field order is preserved, duplicates are collapsed, malformed lines
are skipped, and missing requested fields are reported without invented values.
`parse_meminfo(text, None)` returns all parsed fields sorted by name.
`to_json_dict(&snapshot, include_hostname)` returns an insertion-ordered JSON
map with the original topic names, integer uptime, and UTC second-resolution
`YYYY-MM-DDTHH:MM:SSZ` timestamp. Serialize it with `serde_json::to_string`.

These metrics are Linux-specific, not approximated by cross-platform substitutes.
Without procfs (including on other operating systems), missing reads appear in
`Snapshot.errors` and metrics are omitted. Hostname retrieval failures likewise
produce an error and omit the hostname rather than use a placeholder.

Hardening relative to Python: NaN, infinity, negative uptime and uptime outside
the representable u64 wire range are rejected as collector errors. Memory
integers use i64 rather than arbitrary-precision Python ints; out-of-range lines
are skipped just like malformed lines and requested missing fields are reported.

## Logging

Call `log::init_logger(false)?` once at startup, then
`logger.apply_knobs(&config["knobs"])`. The returned `&'static Logger` also has
`log_to_console` and `set_log_level_debug`; `get_logger()` retrieves it after
initialization. A repeated registration returns `log::SetLoggerError`, not a
second installation. Use the external `log` crate's macros with the default
`system_report` module targets (or explicit `target: "system_report"`). Other
crate targets are filtered, matching the Python application-named logger.

The default level is INFO. Testing mode adds a console handler and enables
DEBUG, as in Python. The facade ceiling stays at DEBUG to allow runtime knobs;
TRACE remains disabled. Explicit extra-console requests may duplicate output,
just as Python's opt-in console handler does.

A nonempty JOURNAL_STREAM chooses the console directly. Otherwise the first
actual socket among `/dev/log` and `/var/run/syslog` is tried. The journald
**forwarding** socket is deliberately excluded. Unix datagram and stream syslog
are supported with LOG_DAEMON priorities and Python-compatible NUL termination.
Console uses **stderr**, preserving the actual behavior of Python's default
`logging.StreamHandler()` (despite the Python comments saying stdout). Both
streams are normally captured by systemd. Formatting preserves local timestamp,
application name, right-aligned module leaf, source line, and severity label.

Unavailable or stale sockets fall back to console. Socket writes have a
one-second timeout. If an established socket fails later, that handler switches
to stderr and retries the current line there; it stays on console for subsequent
records, rather than silently losing logs. It does not automatically reconnect.

## Verification

All builds/tests run on hermes-playground in `docker.io/library/rust:1`.
`cargo test --locked` includes ported Python collector/logging scenarios,
non-finite uptime cases, duplicate/malformed memory fields, real Linux procfs,
real Unix datagram/stream syslog packets, stale/disconnected sockets, formatting,
knobs, filtering, and global logger registration. Socket receives have deadlines.
The tests do not require a production journal, host service, broker, or LAN.
