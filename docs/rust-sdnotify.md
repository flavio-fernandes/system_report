# Rust systemd notifications

`system_report::sdnotify::Notifier` (Unix) ports the Python notifier. Construct
with `Notifier::new()` to capture the three systemd environment variables, or
`Notifier::from_env(&HashMap<String, String>)` for an explicit environment.
Use `enabled()`, `address()`, and `watchdog_interval_secs()` to inspect it;
`ready()`, `watchdog()`, `status(text)`, `stopping()`, and `notify(state)` return
whether a datagram was sent. `close()` releases the cached socket, but permits
later sends. Drop releases it too. No process environment variables are changed.

The watchdog interval is half a positive WATCHDOG_USEC deadline; WATCHDOG_PID
must be absent, empty, or exactly the current PID. Invalid, non-positive, or
out-of-u64-range deadlines disable the watchdog. Unlike Python's arbitrary-size
integers, Rust limits deadlines to u64 microseconds.

sd-notify 0.5 supplies NotifyState protocol serialization. Its global `notify`
API is deliberately not used: it reads the live environment for each call and
appends a newline. A small UnixDatagram adapter preserves captured environment,
exact Python wire bytes, lazy socket reuse, and recovery after send failures.
The crate's watchdog parser also differs for malformed WATCHDOG_PID, so parsing
is kept in the adapter. This is a compatibility layer, not a systemd binding.

`@name` is exposed as `\0name` on all Unix platforms. Linux sends through an
abstract SocketAddr; other Unix platforms return false and log an unsupported
address warning. Filesystem datagram sockets work on other Unix platforms.
Stream sockets are intentionally rejected, as in Python: systemd's notify
protocol is datagram-only. Failures log a warning, close the socket, and return
false; the next call retries. A missing/empty NOTIFY_SOCKET is a silent no-op.

The nine Linux integration tests port all Python test scenarios and add real
stream rejection, retry after disappearance/rebind, close/reopen, Unicode and
custom payloads, invalid paths, and additional watchdog validation. Socket
receives have timeouts and short /tmp paths for macOS/BSD sun_path limits.
Only the real abstract round trip is Linux-gated; off Linux an unsupported-
address test replaces it. Tests never mutate global environment variables.

Validation was executed in rust:1 on the Linux playground: cargo test --locked
and cargo clippy --locked --all-targets -- -D warnings passed. Non-Linux tests
are conditionally compiled but were not executed on a non-Linux host.
