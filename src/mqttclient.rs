//! Outage-tolerant MQTT 3.1.1 publisher. Network I/O belongs to a private thread.
use crate::config::Config;
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Outgoing, Packet, QoS, Transport};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc as channel, oneshot};

type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;
type Reply = mpsc::Sender<bool>;
#[derive(Default)]
struct State {
    connected: bool,
    since: Duration,
    dropped: u64,
    last_log: Option<Duration>,
}
struct Message {
    topic: String,
    payload: Vec<u8>,
    qos: QoS,
    retain: bool,
    reply: Reply,
}
struct Worker {
    tx: channel::Sender<Message>,
    stop: oneshot::Sender<()>,
    join: thread::JoinHandle<()>,
}

pub struct Publisher {
    options: MqttOptions,
    online: String,
    offline: String,
    status: String,
    status_retain: bool,
    qos: QoS,
    retain: bool,
    timeout: Duration,
    recreate: Duration,
    min_delay: Duration,
    max_delay: Duration,
    capacity: usize,
    state: Arc<Mutex<State>>,
    clock: Clock,
    worker: Option<Worker>,
}
fn qos(n: u64) -> QoS {
    match n {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        _ => QoS::ExactlyOnce,
    }
}
impl Publisher {
    pub fn new(cfg: &Config) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let epoch = Instant::now();
        Self::with_clock(cfg, Arc::new(move || epoch.elapsed()))
    }
    /// Inject a monotonic clock for deterministic outage/recreation tests.
    pub fn with_clock(
        cfg: &Config,
        clock: Clock,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let m = &cfg["mqtt"];
        let t = &cfg["topics"];
        let mut options = MqttOptions::new(
            cfg.client_id(),
            m["host"].as_str().unwrap(),
            m["port"].as_u64().unwrap() as u16,
        );
        options.set_clean_session(m["clean_session"].as_bool().unwrap());
        options.set_keep_alive(Duration::from_secs(m["keepalive_secs"].as_u64().unwrap()));
        options.set_last_will(LastWill::new(
            &cfg.status_topic,
            t["payload_offline"].as_str().unwrap(),
            QoS::AtLeastOnce,
            t["status_retain"].as_bool().unwrap(),
        ));
        if let Some(user) = m["username"].as_str().filter(|user| !user.is_empty()) {
            options.set_credentials(user, cfg.password.as_deref().unwrap_or(""));
        }
        if m["tls"]["enabled"].as_bool().unwrap() {
            let tls = &m["tls"];
            let mut builder = native_tls::TlsConnector::builder();
            if let Some(path) = tls["ca_certs"].as_str() {
                // A PEM bundle may hold multiple trust anchors.
                let bytes = std::fs::read(path)?;
                let pem = String::from_utf8(bytes)?;
                for part in pem.split_inclusive("-----END CERTIFICATE-----") {
                    if part.contains("-----BEGIN CERTIFICATE-----") {
                        builder.add_root_certificate(native_tls::Certificate::from_pem(
                            part.as_bytes(),
                        )?);
                    }
                }
                builder.disable_built_in_roots(true);
            }
            if let Some(cert) = tls["certfile"].as_str() {
                let key = tls["keyfile"].as_str().unwrap_or(cert);
                builder.identity(native_tls::Identity::from_pkcs8(
                    &std::fs::read(cert)?,
                    &std::fs::read(key)?,
                )?);
            }
            // paho tls_insecure_set disables hostname checks, not CA validation.
            builder.danger_accept_invalid_hostnames(tls["insecure"].as_bool().unwrap());
            options.set_transport(Transport::Tls(builder.build()?.into()));
        }
        let state = State {
            since: clock(),
            ..State::default()
        };
        Ok(Self {
            options,
            online: t["payload_online"].as_str().unwrap().into(),
            offline: t["payload_offline"].as_str().unwrap().into(),
            status: cfg.status_topic.clone(),
            status_retain: t["status_retain"].as_bool().unwrap(),
            qos: qos(m["qos"].as_u64().unwrap()),
            retain: m["retain"].as_bool().unwrap(),
            timeout: Duration::try_from_secs_f64(m["publish_timeout_secs"].as_f64().unwrap())?,
            recreate: Duration::from_secs(m["recreate_client_after_secs"].as_u64().unwrap()),
            min_delay: Duration::from_secs(m["reconnect_min_delay_secs"].as_u64().unwrap()),
            max_delay: Duration::from_secs(m["reconnect_max_delay_secs"].as_u64().unwrap()),
            capacity: usize::try_from(m["max_queued_messages"].as_u64().unwrap())?,
            state: Arc::new(Mutex::new(state)),
            clock,
            worker: None,
        })
    }
    pub fn connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }
    pub fn dropped(&self) -> u64 {
        self.state.lock().unwrap().dropped
    }
    pub fn disconnected_for(&self) -> Duration {
        let s = self.state.lock().unwrap();
        if s.connected {
            Duration::ZERO
        } else {
            (self.clock)().saturating_sub(s.since)
        }
    }
    /// Returns after spawning, even when DNS or the first connection fails.
    pub fn start(&mut self) -> std::io::Result<()> {
        self.teardown();
        {
            let mut s = self.state.lock().unwrap();
            s.connected = false;
            s.since = (self.clock)();
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        // Channel capacity is deliberately independent of the broker queue limit.
        let (tx, rx) = channel::channel(1);
        let (stop, stop_rx) = oneshot::channel();
        let options = self.options.clone();
        let state = self.state.clone();
        let clock = self.clock.clone();
        let online = (self.status.clone(), self.online.clone(), self.status_retain);
        let limits = (self.min_delay, self.max_delay, self.capacity);
        let join = thread::Builder::new()
            .name("mqtt-publisher".into())
            .spawn(move || {
                runtime.block_on(run(options, state, clock, online, limits, rx, stop_rx))
            })?;
        self.worker = Some(Worker { tx, stop, join });
        Ok(())
    }
    pub fn publish(
        &self,
        topic: &str,
        payload: impl ToString,
        qos_override: Option<QoS>,
        retain: Option<bool>,
    ) -> bool {
        if self.worker.is_none() || !self.connected() {
            let mut s = self.state.lock().unwrap();
            s.dropped = s.dropped.saturating_add(1);
            let now = (self.clock)();
            if s.last_log
                .is_none_or(|last| now.saturating_sub(last) >= Duration::from_secs(300))
            {
                s.last_log = Some(now);
                log::warn!("broker not connected: dropped {} message(s)", s.dropped);
            }
            return false;
        }
        let (reply, rx) = mpsc::channel();
        let message = Message {
            topic: topic.into(),
            payload: payload.to_string().into_bytes(),
            qos: qos_override.unwrap_or(self.qos),
            retain: retain.unwrap_or(self.retain),
            reply,
        };
        if self.worker.as_ref().unwrap().tx.try_send(message).is_err() {
            return false;
        }
        rx.recv_timeout(self.timeout).unwrap_or(false)
    }
    pub fn maintain(&mut self) -> std::io::Result<bool> {
        if self.recreate.is_zero()
            || self.connected()
            || self.worker.is_none()
            || self.disconnected_for() < self.recreate
        {
            return Ok(false);
        }
        log::warn!("no broker connection: rebuilding mqtt client");
        self.start()?;
        Ok(true)
    }
    pub fn stop(&mut self) {
        if self.worker.is_some() && self.connected() {
            self.publish(
                &self.status,
                &self.offline,
                Some(QoS::AtLeastOnce),
                Some(self.status_retain),
            );
        }
        self.teardown();
    }
    fn teardown(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.stop.send(());
            let _ = worker.join.join();
        }
        self.state.lock().unwrap().connected = false;
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run(
    options: MqttOptions,
    state: Arc<Mutex<State>>,
    clock: Clock,
    online: (String, String, bool),
    limits: (Duration, Duration, usize),
    mut rx: channel::Receiver<Message>,
    mut stop: oneshot::Receiver<()>,
) {
    let (client, mut events) = AsyncClient::new(options, 16);
    let (min_delay, max_delay, capacity) = limits;
    let mut delay = min_delay;
    let mut queued: VecDeque<Option<Reply>> = VecDeque::new();
    let mut pending: HashMap<u16, Option<Reply>> = HashMap::new();
    loop {
        tokio::select! {
            _ = &mut stop => {
                if state.lock().unwrap().connected {
                    let _ = client.try_disconnect();
                    let _ = tokio::time::timeout(Duration::from_secs(1), async {
                        loop { match events.poll().await { Ok(Event::Outgoing(Outgoing::Disconnect)) | Err(_) => break, _ => {} } }
                    }).await;
                }
                break;
            }
            Some(m) = rx.recv() => {
                // Like paho, the outgoing queue limit applies only to QoS 1/2.
                if !state.lock().unwrap().connected || (m.qos != QoS::AtMostOnce && capacity != 0 && queued.len() + pending.len() >= capacity) {
                    let _ = m.reply.send(false); continue;
                }
                match client.try_publish(m.topic, m.qos, m.retain, m.payload) {
                    Ok(()) => {
                        if m.qos == QoS::AtMostOnce { let _ = m.reply.send(true); queued.push_back(None); }
                        else { queued.push_back(Some(m.reply)); }
                    }
                    Err(_) => { let _ = m.reply.send(false); }
                }
            }
            event = events.poll() => match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    { let mut s = state.lock().unwrap(); s.connected = true; s.dropped = 0; s.last_log = None; }
                    delay = min_delay;
                    if client.try_publish(&online.0, QoS::AtLeastOnce, online.2, online.1.as_bytes()).is_ok() { queued.push_back(None); }
                }
                Ok(Event::Outgoing(Outgoing::Publish(id))) => {
                    // Retransmitted QoS packets retain their packet id and waiter.
                    if id == 0 { queued.pop_front(); }
                    else if let std::collections::hash_map::Entry::Vacant(entry) = pending.entry(id) { entry.insert(queued.pop_front().flatten()); }
                }
                Ok(Event::Incoming(Packet::PubAck(ack))) => finish(&mut pending, ack.pkid),
                Ok(Event::Incoming(Packet::PubComp(ack))) => finish(&mut pending, ack.pkid),
                Err(_) => {
                    { let mut s = state.lock().unwrap(); if s.connected { s.connected = false; s.since = clock(); } }
                    log::warn!("mqtt connection unavailable; retrying");
                    tokio::select! { _ = &mut stop => break, _ = tokio::time::sleep(delay) => {} }
                    delay = delay.saturating_mul(2).min(max_delay);
                }
                _ => {}
            }
        }
    }
    state.lock().unwrap().connected = false;
}
fn finish(pending: &mut HashMap<u16, Option<Reply>>, id: u16) {
    if let Some(Some(reply)) = pending.remove(&id) {
        let _ = reply.send(true);
    }
}
