use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use system_report::{
    config, consts,
    mqttclient::Publisher,
    reporter::{Publish, Reporter},
    sdnotify::Notifier,
};

#[derive(Default)]
struct Args {
    config: Option<PathBuf>,
    once: bool,
    dry_run: bool,
    print_config: bool,
}
fn args() -> Result<Option<Args>, ()> {
    let mut args = Args::default();
    let mut positional = false;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--") if !positional => positional = true,
            Some("--help" | "-h") if !positional => {
                println!(
                    "Usage: system_report [CONFIG] [--once] [--dry-run] [--print-config]\nPublish Linux memory/uptime to MQTT.\nCONFIG defaults to data/config.yaml in the build checkout.\n--once         Publish one report and exit\n--dry-run      Print messages without network access\n--print-config Print effective configuration with secrets redacted\n--version      Print version"
                );
                return Ok(None);
            }
            Some("--version") if !positional => {
                println!("{} {}", consts::APP_NAME, consts::VERSION);
                return Ok(None);
            }
            Some("--once") if !positional => args.once = true,
            Some("--dry-run") if !positional => args.dry_run = true,
            Some("--print-config") if !positional => args.print_config = true,
            Some(s) if !positional && s.starts_with('-') => return Err(()),
            _ if args.config.is_none() => args.config = Some(arg.into()),
            _ => return Err(()),
        }
    }
    Ok(Some(args))
}
struct DryRun;
impl Publish for DryRun {
    fn connected(&self) -> bool {
        true
    }
    fn maintain(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn publish(&mut self, _: &str, _: &str) -> bool {
        true
    }
}
fn wait(stop: &AtomicBool, duration: Duration) {
    let start = Instant::now();
    while !stop.load(Ordering::Relaxed) && start.elapsed() < duration {
        std::thread::sleep(
            duration
                .saturating_sub(start.elapsed())
                .min(Duration::from_millis(50)),
        );
    }
}
fn main() {
    std::process::exit(run());
}
fn run() -> i32 {
    let args = match args() {
        Ok(Some(args)) => args,
        Ok(None) => return 0,
        Err(()) => {
            eprintln!("invalid arguments; see --help");
            return 2;
        }
    };
    let logger = system_report::log::init_logger(false).expect("logger initialized once");
    let cfg = match config::load(args.config.as_deref(), None, None) {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    logger.apply_knobs(&cfg["knobs"]);
    cfg.log_warnings(|warning| log::warn!("{warning}"));
    if args.print_config {
        println!(
            "{}",
            serde_json::to_string_pretty(&cfg.redacted()).expect("validated config")
        );
        return 0;
    }
    if args.dry_run {
        let mut reporter = Reporter::new(cfg, DryRun, None);
        let snapshot = reporter.collect();
        for error in &snapshot.errors {
            log::warn!("collection problem: {error}");
        }
        return match reporter.messages_for(&snapshot) {
            Ok(messages) => {
                for (topic, payload) in messages {
                    println!("{topic} {payload}");
                }
                0
            }
            Err(error) => {
                log::error!("{error}");
                1
            }
        };
    }
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        if signal_hook::flag::register(signal, stop.clone()).is_err() {
            log::error!("cannot register signal handler");
            return 1;
        }
    }
    let publisher = match Publisher::new(&cfg) {
        Ok(p) => p,
        Err(_) => {
            log::error!("cannot initialize MQTT publisher; check TLS configuration");
            return 2;
        }
    };
    log::info!(
        "{} {} starting: host={} interval={}s",
        consts::APP_NAME,
        consts::VERSION,
        cfg.hostname,
        cfg["report"]["interval_secs"].as_u64().unwrap()
    );
    let connect_wait =
        Duration::from_secs(cfg["mqtt"]["reconnect_max_delay_secs"].as_u64().unwrap());
    let mut reporter = Reporter::new(cfg, publisher, Some(Notifier::new()));
    if reporter.publisher.start().is_err() {
        log::error!("cannot start MQTT worker");
        return 1;
    }
    reporter.notifier.as_mut().unwrap().ready();
    let rc = if args.once {
        let start = Instant::now();
        while !reporter.publisher.connected()
            && start.elapsed() < connect_wait
            && !stop.load(Ordering::Relaxed)
        {
            reporter.notifier.as_mut().unwrap().watchdog();
            let delay = reporter
                .notifier
                .as_ref()
                .unwrap()
                .watchdog_interval_secs()
                .unwrap_or(0.25)
                .min(0.25);
            wait(
                &stop,
                Duration::from_secs_f64(delay).min(connect_wait.saturating_sub(start.elapsed())),
            );
        }
        if stop.load(Ordering::Relaxed) {
            0
        } else {
            match reporter.report_once() {
                Ok(_) => 0,
                Err(error) => {
                    log::error!("{error}");
                    1
                }
            }
        }
    } else {
        reporter.run(
            || stop.load(Ordering::Relaxed),
            |duration| wait(&stop, duration),
        )
    };
    reporter.notifier.as_mut().unwrap().stopping();
    reporter.publisher.stop();
    reporter.notifier.as_mut().unwrap().close();
    log::info!("{} stopped (rc={rc})", consts::APP_NAME);
    rc
}
