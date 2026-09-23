use std::{cell::Cell, rc::Rc, time::Duration};
use system_report::{
    config::Config,
    consts,
    reporter::{Publish, Reporter},
};
#[derive(Default)]
struct Fake {
    connected: bool,
    fail: bool,
    calls: u32,
    messages: Vec<(String, String)>,
    slow: Option<Rc<Cell<u64>>>,
}
impl Publish for Fake {
    fn connected(&self) -> bool {
        self.connected
    }
    fn maintain(&mut self) -> std::io::Result<()> {
        self.calls += 1;
        if self.fail {
            Err(std::io::Error::other("boom"))
        } else {
            Ok(())
        }
    }
    fn publish(&mut self, t: &str, p: &str) -> bool {
        if !self.connected {
            return false;
        }
        self.messages.push((t.into(), p.into()));
        if let Some(c) = &self.slow {
            c.set(c.get() + 1000);
        }
        true
    }
}
fn make(yaml: &str, connected: bool, bad_uptime: bool) -> (Reporter<Fake>, Rc<Cell<u64>>) {
    let cfg = Config::from_yaml(yaml, "testhost", &Default::default()).unwrap();
    let clock = Rc::new(Cell::new(1000));
    let c = clock.clone();
    (
        Reporter::with_io(
            cfg,
            Fake {
                connected,
                ..Default::default()
            },
            None,
            move || Duration::from_secs(c.get()),
            move |path| {
                if path == consts::PROC_UPTIME {
                    if bad_uptime {
                        Err(std::io::Error::other("nope"))
                    } else {
                        Ok("93120.42 180000.11\n".into())
                    }
                } else {
                    Ok("MemAvailable: 457392 kB\nSlab: 61204 kB\n".into())
                }
            },
        ),
        clock,
    )
}
const CFG: &str =
    "report: {interval_secs: 600}\nmetrics: {meminfo: {fields: [MemAvailable, Slab]}}";
#[test]
fn ordered_plain_and_json() {
    let (mut r, _) = make(CFG, true, false);
    assert_eq!(r.report_once().unwrap(), 4);
    let m = &r.publisher.messages;
    assert_eq!(
        m.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        [
            "/testhost/oper_uptime_minutes",
            "/testhost/oper_state/mem_available_kb",
            "/testhost/oper_state/slab_kb",
            "/testhost/oper_state/json"
        ]
    );
    assert_eq!(m[1].1, "457392");
    let json: serde_json::Value = serde_json::from_str(&m[3].1).unwrap();
    assert_eq!(json["uptime_minutes"], 1552);
    assert_eq!(json["mem_available_kb"], 457392);
}
#[test]
fn output_switches_and_partial_failure() {
    for (yaml, json_only, no_json, no_uptime, no_mem, bad) in [
        (
            "report: {publish_plain: false}",
            true,
            false,
            false,
            false,
            false,
        ),
        (
            "report: {publish_json: false}",
            false,
            true,
            false,
            false,
            false,
        ),
        (
            "metrics: {uptime_minutes: {enabled: false}}",
            false,
            false,
            true,
            false,
            false,
        ),
        (
            "metrics: {meminfo: {enabled: false}}",
            false,
            false,
            false,
            true,
            false,
        ),
        (CFG, false, false, true, false, true),
    ] {
        let (mut r, _) = make(yaml, true, bad);
        r.report_once().unwrap();
        let m = &r.publisher.messages;
        if json_only {
            assert_eq!(m.len(), 1);
            assert_eq!(m[0].0, r.cfg.json_topic);
        }
        if no_json {
            assert!(!m.iter().any(|(t, _)| t == &r.cfg.json_topic));
        }
        if no_uptime {
            assert!(!m.iter().any(|(t, _)| t == &r.cfg.uptime_topic));
        }
        if no_mem {
            assert_eq!(m.len(), 2);
        }
        if bad {
            assert!(m.iter().any(|(t, _)| t.ends_with("mem_available_kb")));
        }
    }
}
#[test]
fn immediate_then_interval_and_no_catch_up() {
    let (mut r, c) = make(CFG, true, false);
    assert!(r.tick());
    assert_eq!(r.reports, 1);
    c.set(1599);
    r.tick();
    assert_eq!(r.reports, 1);
    c.set(1601);
    r.tick();
    assert_eq!(r.reports, 2);
    c.set(10000);
    r.tick();
    r.tick();
    assert_eq!(r.reports, 3);
    assert_eq!(r.next_report_at(), Duration::from_secs(10600));
}
#[test]
fn delayed_first_report() {
    let (mut r, c) = make(
        "report: {interval_secs: 600, report_on_start: false}",
        true,
        false,
    );
    r.tick();
    assert_eq!(r.reports, 0);
    c.set(1600);
    r.tick();
    assert_eq!(r.reports, 1);
}
#[test]
fn first_connection_waits_but_later_outage_skips() {
    let (mut r, c) = make(CFG, false, false);
    for _ in 0..3 {
        r.tick();
        c.set(c.get() + 1);
    }
    assert_eq!(r.reports, 0);
    r.publisher.connected = true;
    r.tick();
    assert_eq!(r.reports, 1);
    r.publisher.connected = false;
    c.set(1603);
    r.tick();
    r.publisher.connected = true;
    c.set(1604);
    r.tick();
    assert_eq!(r.reports, 1);
    c.set(2203);
    r.tick();
    assert_eq!(r.reports, 2);
}
#[test]
fn failures_exhaust_and_reset() {
    let (mut r, _) = make(CFG, true, false);
    r.publisher.fail = true;
    for _ in 1..consts::MAX_CONSECUTIVE_CYCLE_FAILURES {
        assert!(r.tick());
    }
    assert!(!r.tick());
    r.publisher.fail = false;
    assert!(r.tick());
    r.publisher.fail = true;
    for _ in 1..consts::MAX_CONSECUTIVE_CYCLE_FAILURES {
        assert!(r.tick());
    }
    assert!(!r.tick());
}
#[test]
fn sleep_bounds_and_watchdog() {
    let (mut r, c) = make(CFG, false, false);
    assert_eq!(r.sleep_secs(None).as_secs_f64(), consts::PENDING_RETRY_SECS);
    assert_eq!(r.sleep_secs(Some(0.1)).as_secs_f64(), 0.1);
    r.publisher.connected = true;
    r.tick();
    assert_eq!(r.sleep_secs(None).as_secs_f64(), consts::LOOP_TICK_MAX_SECS);
    assert_eq!(r.sleep_secs(Some(1.5)).as_secs_f64(), 1.5);
    c.set(1598);
    assert_eq!(r.sleep_secs(None).as_secs(), 2);
}
#[test]
fn run_stop_and_failure_exit() {
    let (mut r, _) = make(CFG, true, false);
    assert_eq!(r.run(|| true, |_| panic!("must not sleep")), 0);
    assert_eq!(r.publisher.calls, 0);
    r.publisher.fail = true;
    assert_eq!(r.run(|| false, |_| {}), 1);
    assert_eq!(r.publisher.calls, consts::MAX_CONSECUTIVE_CYCLE_FAILURES);
}
#[test]
fn slow_report_schedules_after_completion() {
    let (mut r, c) = make(CFG, true, false);
    r.publisher.slow = Some(c.clone());
    r.tick();
    assert_eq!(r.next_report_at(), Duration::from_secs(c.get() + 600));
    r.tick();
    assert_eq!(r.reports, 1);
}
