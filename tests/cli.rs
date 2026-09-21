use std::{
    process::{Command, Stdio},
    time::Duration,
};
fn cmd() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_system_report"));
    c.env_remove("SYSTEM_REPORT_MQTT_PASSWORD")
        .env_remove("NOTIFY_SOCKET")
        .env_remove("WATCHDOG_USEC");
    c
}
#[test]
fn cli_modes_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.yaml");
    std::fs::write(&cfg, "mqtt: {host: invalid.invalid, password: test-only-secret}\nmetrics: {meminfo: {fields: [MemAvailable]}}").unwrap();
    for arg in ["--help", "--version"] {
        assert!(cmd().arg(arg).status().unwrap().success());
    }
    for args in [
        vec!["--unknown"],
        vec!["a", "b"],
        vec!["/nonexistent/config.yaml"],
    ] {
        assert_eq!(cmd().args(args).output().unwrap().status.code(), Some(2));
    }
    let out = cmd().arg(&cfg).arg("--print-config").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("<redacted>"));
    assert!(!text.contains("test-only-secret"));
    let out = cmd().arg(&cfg).arg("--dry-run").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("/oper_uptime_minutes "));
    assert!(text.contains("/oper_state/json {"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("mqtt connection"));
}
#[test]
fn binary_notifies_ready_watchdog_and_stopping_offline() {
    use std::os::unix::net::UnixDatagram;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.yaml");
    let socket = dir.path().join("notify.sock");
    std::fs::write(&cfg, "mqtt: {host: 127.0.0.1, port: 1, reconnect_min_delay_secs: 1, reconnect_max_delay_secs: 1}").unwrap();
    let receiver = UnixDatagram::bind(&socket).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut child = cmd()
        .arg(&cfg)
        .env("NOTIFY_SOCKET", &socket)
        .env("WATCHDOG_USEC", "200000")
        .env_remove("WATCHDOG_PID")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut buf = [0; 1024];
    let mut ready = false;
    let mut watchdog = false;
    for _ in 0..5 {
        let n = receiver.recv(&mut buf).unwrap();
        ready |= &buf[..n] == b"READY=1";
        watchdog |= &buf[..n] == b"WATCHDOG=1";
        if ready && watchdog {
            break;
        }
    }
    // kill(1) sends SIGTERM, not SIGKILL; the registered handler must allow cleanup.
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let mut stopping = false;
    for _ in 0..10 {
        let n = receiver.recv(&mut buf).unwrap();
        if &buf[..n] == b"STOPPING=1" {
            stopping = true;
            break;
        }
    }
    assert!(child.wait().unwrap().success());
    assert!(ready && watchdog && stopping);
}
#[test]
fn once_is_bounded_without_a_broker() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.yaml");
    std::fs::write(&cfg, "mqtt: {host: 127.0.0.1, port: 1, reconnect_min_delay_secs: 1, reconnect_max_delay_secs: 1}").unwrap();
    let start = std::time::Instant::now();
    let out = cmd().arg(cfg).arg("--once").output().unwrap();
    assert!(out.status.success());
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(String::from_utf8_lossy(&out.stderr).contains("report #1:"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("report #2:"));
}
