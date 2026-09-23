#![cfg(unix)]
use std::collections::HashMap;
use std::os::unix::net::{UnixDatagram, UnixListener};
use std::time::Duration;
use system_report::sdnotify::Notifier;

fn notifier(values: &[(&str, &str)]) -> Notifier {
    let env: HashMap<String, String> = values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Notifier::from_env(&env)
}

fn socket_dir() -> tempfile::TempDir {
    // macOS's default temporary directory may exceed sun_path's 104-byte limit.
    tempfile::Builder::new()
        .prefix("sr-")
        .tempdir_in("/tmp")
        .unwrap()
}

fn receive(socket: &UnixDatagram, expected: &str) {
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut buffer = [0; 512];
    let len = socket.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..len], expected.as_bytes());
}

#[test]
fn notifications_reach_socket_and_close_is_reusable() {
    let dir = socket_dir();
    let path = dir.path().join("n.sock");
    let path = path.to_str().unwrap();
    assert!(path.len() < 100);
    let socket = UnixDatagram::bind(path).unwrap();
    let mut n = notifier(&[("NOTIFY_SOCKET", path)]);
    assert!(n.enabled());
    assert!(n.ready());
    receive(&socket, "READY=1");
    assert!(n.status("all good — café"));
    receive(&socket, "STATUS=all good — café");
    assert!(n.stopping());
    receive(&socket, "STOPPING=1");
    assert!(n.notify("X=one\nY=two"));
    receive(&socket, "X=one\nY=two");
    n.close();
    n.close();
    assert!(n.ready());
    receive(&socket, "READY=1");
}

#[test]
fn without_socket_everything_is_noop() {
    for env in [
        vec![],
        vec![("NOTIFY_SOCKET", "")],
        vec![("WATCHDOG_USEC", "2")],
    ] {
        let mut n = notifier(&env);
        assert!(!n.enabled());
        assert_eq!(n.address(), None);
        assert!(!n.ready());
        assert!(!n.watchdog());
        assert!(!n.status("ignored"));
        assert!(!n.stopping());
        n.close();
    }
    assert_eq!(notifier(&[]).watchdog_interval_secs(), None);
}

#[test]
fn watchdog_only_sends_when_armed() {
    let dir = socket_dir();
    let path = dir.path().join("n.sock");
    let path = path.to_str().unwrap();
    let socket = UnixDatagram::bind(path).unwrap();
    let mut silent = notifier(&[("NOTIFY_SOCKET", path)]);
    assert!(!silent.watchdog());
    socket.set_nonblocking(true).unwrap();
    assert_eq!(
        socket.recv(&mut [0; 64]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    socket.set_nonblocking(false).unwrap();
    let mut armed = notifier(&[("NOTIFY_SOCKET", path), ("WATCHDOG_USEC", "180000000")]);
    assert_eq!(armed.watchdog_interval_secs(), Some(90.0));
    assert!(armed.watchdog());
    receive(&socket, "WATCHDOG=1");
}

#[test]
fn watchdog_pid_and_interval_validation() {
    let pid = std::process::id().to_string();
    for value in ["", &pid] {
        assert_eq!(
            notifier(&[("WATCHDOG_PID", value), ("WATCHDOG_USEC", "3")]).watchdog_interval_secs(),
            Some(0.0000015)
        );
    }
    for value in ["garbled", "0", "-1", " 1 "] {
        let mut n = notifier(&[("WATCHDOG_PID", value), ("WATCHDOG_USEC", "180000000")]);
        assert_eq!(n.watchdog_interval_secs(), None);
        assert!(!n.watchdog());
    }
    let other_pid = (std::process::id() + 1).to_string();
    assert_eq!(
        notifier(&[("WATCHDOG_PID", &other_pid), ("WATCHDOG_USEC", "1")]).watchdog_interval_secs(),
        None
    );
    for value in ["not-a-number", "", "0", "-1", "1.5", "18446744073709551616"] {
        assert_eq!(
            notifier(&[("WATCHDOG_USEC", value)]).watchdog_interval_secs(),
            None
        );
    }
    assert_eq!(
        notifier(&[("WATCHDOG_USEC", " +2000000 ")]).watchdog_interval_secs(),
        Some(1.0)
    );
}

#[test]
fn abstract_translation_is_platform_independent() {
    assert_eq!(
        notifier(&[("NOTIFY_SOCKET", "@sysrep")]).address(),
        Some("\0sysrep")
    );
    assert_eq!(
        notifier(&[("NOTIFY_SOCKET", "/run/x.sock")]).address(),
        Some("/run/x.sock")
    );
    assert_eq!(notifier(&[]).address(), None);
}

#[test]
#[cfg(target_os = "linux")]
fn abstract_datagram_round_trip() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::SocketAddr;
    let name = format!("system-report-test-{}", std::process::id());
    let socket =
        UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
    let mut n = notifier(&[("NOTIFY_SOCKET", &format!("@{name}"))]);
    assert!(n.ready());
    receive(&socket, "READY=1");
}

#[test]
#[cfg(not(target_os = "linux"))]
fn unsupported_abstract_address_fails_harmlessly() {
    assert!(!notifier(&[("NOTIFY_SOCKET", "@sysrep")]).ready());
}

#[test]
fn dead_socket_retries_when_listener_appears_and_restarts() {
    let dir = socket_dir();
    let path = dir.path().join("n.sock");
    let mut n = notifier(&[("NOTIFY_SOCKET", path.to_str().unwrap())]);
    assert!(!n.ready());
    let socket = UnixDatagram::bind(&path).unwrap();
    assert!(n.ready());
    receive(&socket, "READY=1");
    drop(socket);
    assert!(!n.ready());
    std::fs::remove_file(&path).unwrap();
    let socket = UnixDatagram::bind(&path).unwrap();
    assert!(n.ready());
    receive(&socket, "READY=1");
}

#[test]
fn stream_socket_is_rejected_without_connecting() {
    let dir = socket_dir();
    let path = dir.path().join("n.sock");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut n = notifier(&[("NOTIFY_SOCKET", path.to_str().unwrap())]);
    assert!(!n.ready());
    assert!(!n.stopping());
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn invalid_socket_path_fails_harmlessly() {
    for path in [
        "/invalid\0path".to_string(),
        format!("/{}", "x".repeat(200)),
    ] {
        assert!(!notifier(&[("NOTIFY_SOCKET", &path)]).ready());
    }
}
