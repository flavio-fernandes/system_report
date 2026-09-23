# Rust MQTT publisher

`mqttclient::Publisher::new(&Config)` configures rumqttc MQTT 3.1.1 and TLS.
`start(&mut self)` spawns a private network thread and does not wait for a broker.
Use `publish(topic, payload, Option<rumqttc::QoS>, Option<bool>)` for reports;
`None` selects the configured QoS/retain setting. Payloads implement `ToString`:
serialize structured collector data to JSON before publishing.

Call `maintain(&mut self)` periodically to rebuild after the configured outage
threshold (`Ok(true)` means rebuilt). Persistent sessions disable recreation so
unfinished exchanges survive reconnects; state is in memory, not on disk. `connected()`, `dropped()`, and
`disconnected_for()` expose state. `stop()` publishes the QoS 1 offline status
when connected, disconnects, and joins the worker; it is idempotent. Drop also
calls stop. Construction/start/maintain errors should be handled by main.

Wire contract: configured topics and payloads are used verbatim; online, offline,
and last will always use QoS 1 and `topics.status_retain`. Metrics use MQTT QoS
and retain defaults or per-call overrides. Disconnected reports are dropped,
not buffered. Reconnect uses exponential backoff from min to max delay and
reannounces online after success. QoS 1/2 publication waits for acknowledgment
up to the configured timeout; a timeout is not cancellation (the broker may
still receive the publication). QoS 0 waits for the outgoing socket write within
the same timeout; this is not proof of broker receipt. The application queue cap
applies only to QoS 1/2, but rumqttc's transport flow control (currently 100
in-flight packets) can also stall QoS 0. A stalled write returns false on timeout;
it may still be sent when acknowledgements resume. Report logs count publishes
completed within their timeout, not total broker deliveries.

On connection loss, outstanding callers fail promptly. Clean sessions discard
old buffered work; persistent sessions preserve unfinished PUBLISH/PUBREL
exchanges when the broker reports a resumed session. If the broker has forgotten
the session, old work is discarded. No new reports are queued while disconnected.

TLS uses native-tls/system trust or a supplied CA bundle, optional client
certificate/key, and the configured hostname-verification override. Client keys
must be unencrypted PKCS#8 PEM. When `keyfile` is omitted, `certfile` may contain
both certificate chain and key, in either order. See the upgrade recipe for
legacy PKCS#1 conversion; malformed keys fail with a non-secret diagnostic. No live LAN
broker is needed for tests: `tests/mqttclient.rs` runs scripted loopback MQTT
peers to inspect CONNECT/LWT, authentication, online/offline, QoS 0/1/2, retain,
refusal, disconnect/reconnect, timeout, queue limits and clean DISCONNECT.
The injected clock tests client recreation. TLS tests cover setup and missing
material and Linux identity loading with disposable certificates; a full TLS handshake and broker-delivered LWT are not integration-tested.

Validation on the playground: `cargo test --locked`,
`cargo clippy --locked --all-targets -- -D warnings`, and `cargo fmt --all -- --check`.
