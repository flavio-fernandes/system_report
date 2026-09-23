//! Monotonic, outage-tolerant reporting. Inject I/O and time in tests.
use crate::{
    collectors::{self, CollectOptions, Snapshot},
    config::{Config, ConfigError},
    consts, mqttclient,
    sdnotify::Notifier,
};
use std::{
    io,
    time::{Duration, Instant},
};

pub trait Publish {
    fn connected(&self) -> bool;
    fn maintain(&mut self) -> io::Result<()>;
    fn publish(&mut self, topic: &str, payload: &str) -> bool;
}
impl Publish for mqttclient::Publisher {
    fn connected(&self) -> bool {
        self.connected()
    }
    fn maintain(&mut self) -> io::Result<()> {
        self.maintain().map(|_| ())
    }
    fn publish(&mut self, topic: &str, payload: &str) -> bool {
        mqttclient::Publisher::publish(self, topic, payload, None, None)
    }
}
type Reader = Box<dyn FnMut(&str) -> io::Result<String>>;
pub struct Reporter<P> {
    pub cfg: Config,
    pub publisher: P,
    pub notifier: Option<Notifier>,
    clock: Box<dyn Fn() -> Duration>,
    reader: Reader,
    interval: Duration,
    next: Duration,
    first_pending: bool,
    failures: u32,
    pub reports: u64,
}
impl<P: Publish> Reporter<P> {
    pub fn new(cfg: Config, publisher: P, notifier: Option<Notifier>) -> Self {
        let epoch = Instant::now();
        Self::with_io(
            cfg,
            publisher,
            notifier,
            move || epoch.elapsed(),
            |path| std::fs::read_to_string(path),
        )
    }
    pub fn with_io(
        cfg: Config,
        publisher: P,
        notifier: Option<Notifier>,
        clock: impl Fn() -> Duration + 'static,
        reader: impl FnMut(&str) -> io::Result<String> + 'static,
    ) -> Self {
        let interval = Duration::from_secs(cfg["report"]["interval_secs"].as_u64().unwrap());
        let first_pending = cfg["report"]["report_on_start"].as_bool().unwrap();
        let next = clock().saturating_add(if first_pending {
            Duration::ZERO
        } else {
            interval
        });
        Self {
            cfg,
            publisher,
            notifier,
            clock: Box::new(clock),
            reader: Box::new(reader),
            interval,
            next,
            first_pending,
            failures: 0,
            reports: 0,
        }
    }
    pub fn collect(&mut self) -> Snapshot {
        let fields = self.cfg.meminfo_fields();
        let refs: Vec<_> = fields.iter().map(String::as_str).collect();
        collectors::collect_with_reader(
            &CollectOptions {
                fields: &refs,
                want_uptime: self.cfg.uptime_enabled(),
                want_meminfo: self.cfg.meminfo_enabled(),
                hostname: Some(&self.cfg.hostname),
                now: None,
            },
            &mut self.reader,
        )
    }
    pub fn messages_for(&self, snapshot: &Snapshot) -> Result<Vec<(String, String)>, ConfigError> {
        let mut messages = Vec::new();
        if self.cfg["report"]["publish_plain"].as_bool().unwrap() {
            if self.cfg.uptime_enabled()
                && let Some(value) = snapshot.uptime_minutes
            {
                messages.push((self.cfg.uptime_topic.clone(), value.to_string()));
            }
            if self.cfg.meminfo_enabled() {
                for (field, value) in &snapshot.meminfo {
                    messages.push((self.cfg.meminfo_topic(field)?, value.to_string()));
                }
            }
        }
        if self.cfg["report"]["publish_json"].as_bool().unwrap() {
            messages.push((
                self.cfg.json_topic.clone(),
                serde_json::Value::Object(collectors::to_json_dict(snapshot, true)).to_string(),
            ));
        }
        Ok(messages)
    }
    pub fn report_once(&mut self) -> Result<usize, ConfigError> {
        let snapshot = self.collect();
        for error in &snapshot.errors {
            log::warn!("collection problem: {error}");
        }
        let messages = self.messages_for(&snapshot)?;
        let published = messages
            .iter()
            .filter(|(topic, payload)| self.publisher.publish(topic, payload))
            .count();
        self.reports += 1;
        log::info!(
            "report #{}: completed {}/{} publish(es) within timeout",
            self.reports,
            published,
            messages.len()
        );
        if let Some(notifier) = &mut self.notifier {
            notifier.status(&format!(
                "reports={} completed={}/{} connected={}",
                self.reports,
                published,
                messages.len(),
                self.publisher.connected()
            ));
        }
        Ok(published)
    }
    pub fn next_report_at(&self) -> Duration {
        self.next
    }
    pub fn seconds_until_due(&self) -> Duration {
        self.next.saturating_sub((self.clock)())
    }
    fn cycle(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.publisher.maintain()?;
        if self.seconds_until_due().is_zero() {
            if self.publisher.connected() {
                self.report_once()?;
                self.first_pending = false;
                // Sample AFTER publishing: a slow report cannot create catch-up bursts.
                self.next = (self.clock)().saturating_add(self.interval);
            } else if !self.first_pending {
                self.next = (self.clock)().saturating_add(self.interval);
            }
        }
        Ok(())
    }
    pub fn tick(&mut self) -> bool {
        match self.cycle() {
            Ok(()) => {
                self.failures = 0;
                true
            }
            Err(error) => {
                self.failures += 1;
                log::error!(
                    "report loop failure ({}/{}): {error}",
                    self.failures,
                    consts::MAX_CONSECUTIVE_CYCLE_FAILURES
                );
                self.failures < consts::MAX_CONSECUTIVE_CYCLE_FAILURES
            }
        }
    }
    pub fn sleep_secs(&self, watchdog: Option<f64>) -> Duration {
        let mut wait = self
            .seconds_until_due()
            .min(Duration::from_secs_f64(consts::LOOP_TICK_MAX_SECS));
        if wait.is_zero() {
            wait = Duration::from_secs_f64(consts::PENDING_RETRY_SECS);
        }
        // Unlike the Python floor of one second, honour sub-second watchdogs too.
        if let Some(secs) = watchdog.filter(|s| s.is_finite() && *s > 0.0) {
            wait = wait.min(Duration::from_secs_f64(secs));
        }
        wait
    }
    pub fn run(&mut self, stopped: impl Fn() -> bool, mut wait: impl FnMut(Duration)) -> i32 {
        while !stopped() {
            if !self.tick() {
                return 1;
            }
            let watchdog = self.notifier.as_mut().and_then(|n| {
                n.watchdog();
                n.watchdog_interval_secs()
            });
            wait(self.sleep_secs(watchdog));
        }
        0
    }
}
