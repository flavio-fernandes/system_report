# Rust MQTT publisher

`mqttclient::Publisher::new(&Config)` configures rumqttc MQTT 3.1.1 and TLS.
`start(&mut self)` spawns a private network thread and does not wait for a broker.
Use `publish(topic, payload, Option<rumqttc::QoS>, Option<bool>)` for reports;
`None` selects the configured QoS/retain setting. Payloads implement `ToString`:
serialize structured collector data to JSON before publishing.

Call `maintain(&mut self)` periodically to rebuild after the configured outage
threshold (`Ok(true)` means rebuilt). `connected()`, `dropped()`, and
`disconnected_for()` expose state. `stop()` publishes the QoS 1 offline status
when connected, disconnects, and joins the worker; it is idempotent. Drop also
calls stop. Construction/start/maintain errors should be handled by main.

Wire contract: configured topics and payloads are used verbatim; online, offline,
and last will always use QoS 1 and `topics.status_retain`. Metrics use MQTT QoS
and retain defaults or per-call overrides. Disconnected reports are dropped,
not buffered. Reconnect uses exponential backoff from min to max delay and
reannounces online after success. QoS 1/2 publication waits for acknowledgment
up to the configured timeout; a timeout is not cancellation (the broker may
still receive the publication). QoS 0 reports success on local queue acceptance.
A full QoS 1/2 queue does not prevent QoS 0 reports.

TLS uses native-tls/system trust or a supplied CA bundle, optional client
certificate/key, and the configured hostname-verification override. No live LAN
broker is needed for tests: `tests/mqttclient.rs` runs scripted loopback MQTT
peers to inspect CONNECT/LWT, authentication, online/offline, QoS 0/1/2, retain,
refusal, disconnect/reconnect, timeout, queue limits and clean DISCONNECT.
The injected clock tests client recreation. TLS tests cover setup and missing
material; a full TLS handshake and broker-delivered LWT are not integration-tested.

Validation on the playground: `cargo test --locked`,
`cargo clippy --locked --all-targets -- -D warnings`, and `cargo fmt --all -- --check`.
