//! A tiny scripted MQTT broker: wire assertions, no production broker or LAN access.
use rumqttc::QoS;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use system_report::{config::Config, mqttclient::Publisher};
fn listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").unwrap()
}
fn cfg(port: u16, extra: &str) -> Config {
    Config::from_yaml(
        &format!("mqtt:\n  host: 127.0.0.1\n  port: {port}\n  publish_timeout_secs: 0.2\n{extra}"),
        "testhost",
        &HashMap::new(),
    )
    .unwrap()
}
fn accept(l: &TcpListener) -> TcpStream {
    let (s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s
}
fn packet(s: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut byte = [0];
    s.read_exact(&mut byte).unwrap();
    let header = byte[0];
    let (mut len, mut scale) = (0, 1);
    loop {
        s.read_exact(&mut byte).unwrap();
        len += usize::from(byte[0] & 127) * scale;
        if byte[0] & 128 == 0 {
            break;
        }
        scale *= 128;
    }
    let mut body = vec![0; len];
    s.read_exact(&mut body).unwrap();
    (header, body)
}
fn field(bytes: &[u8], offset: &mut usize) -> Vec<u8> {
    let len = u16::from_be_bytes([bytes[*offset], bytes[*offset + 1]]) as usize;
    *offset += 2;
    let result = bytes[*offset..*offset + len].to_vec();
    *offset += len;
    result
}
fn handshake(s: &mut TcpStream) -> Vec<u8> {
    let (h, b) = packet(s);
    assert_eq!(h, 0x10);
    s.write_all(&[0x20, 2, 0, 0]).unwrap();
    b
}
fn publication(s: &mut TcpStream, topic: &str, payload: &str, qos: u8, retain: bool, ack: bool) {
    let (h, b) = packet(s);
    assert_eq!(h, 0x30 | (qos << 1) | u8::from(retain));
    let mut offset = 0;
    assert_eq!(field(&b, &mut offset), topic.as_bytes());
    let id = if qos > 0 {
        let id = [b[offset], b[offset + 1]];
        offset += 2;
        Some(id)
    } else {
        None
    };
    assert_eq!(&b[offset..], payload.as_bytes());
    if let Some(id) = id.filter(|_| ack) {
        if qos == 1 {
            s.write_all(&[0x40, 2, id[0], id[1]]).unwrap();
        } else {
            s.write_all(&[0x50, 2, id[0], id[1]]).unwrap();
            assert_eq!(packet(s), (0x62, id.to_vec()));
            s.write_all(&[0x70, 2, id[0], id[1]]).unwrap();
        }
    }
}
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < until, "condition timed out");
        thread::sleep(Duration::from_millis(5));
    }
}
const STATUS: &str = "/testhost/oper_state/status";
#[test]
fn wire_defaults_lwt_online_qos_overrides_and_goodbye() {
    let l = listener();
    let mut p = Publisher::new(&cfg(l.local_addr().unwrap().port(), "")).unwrap();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        let b = handshake(&mut s);
        let mut offset = 0;
        assert_eq!(field(&b, &mut offset), b"MQTT");
        assert_eq!(b[offset], 4);
        assert_eq!(b[offset + 1], 0x2e);
        assert_eq!(&b[offset + 2..offset + 4], &[0, 60]);
        offset += 4;
        assert_eq!(field(&b, &mut offset), b"system_report_testhost");
        assert_eq!(field(&b, &mut offset), STATUS.as_bytes());
        assert_eq!(field(&b, &mut offset), b"offline");
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, "/t/mem", "42", 0, false, true);
        publication(&mut s, "/t/mem", "43", 1, true, true);
        publication(&mut s, "/t/mem", "44", 2, false, true);
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    wait(|| p.connected());
    assert!(p.publish("/t/mem", 42, None, None));
    assert!(p.publish("/t/mem", 43, Some(QoS::AtLeastOnce), Some(true)));
    assert!(p.publish("/t/mem", 44, Some(QoS::ExactlyOnce), None));
    assert!(!p.maintain().unwrap());
    p.stop();
    p.stop();
    assert!(!p.connected());
    server.join().unwrap();
}
#[test]
fn credentials_custom_topics_and_configured_flags() {
    let l = listener();
    let config = cfg(
        l.local_addr().unwrap().port(),
        "  username: bob\n  password: synthetic-test-only\n  client_id: custom\n  clean_session: false\n  qos: 1\n  retain: true\n  keepalive_secs: 90\ntopics:\n  prefix: /custom\n  payload_online: up\n  payload_offline: down\n  status_retain: false\n",
    );
    let mut p = Publisher::new(&config).unwrap();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        let b = handshake(&mut s);
        assert_eq!(b[7], 0xcc);
        assert_eq!(&b[8..10], &[0, 90]);
        let mut i = 10;
        for expected in [
            "custom",
            "/custom/oper_state/status",
            "down",
            "bob",
            "synthetic-test-only",
        ] {
            assert_eq!(field(&b, &mut i), expected.as_bytes());
        }
        publication(&mut s, "/custom/oper_state/status", "up", 1, false, true);
        publication(&mut s, "/t", "42", 1, true, true);
        publication(&mut s, "/custom/oper_state/status", "down", 1, false, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    wait(|| p.connected());
    assert!(p.publish("/t", 42, None, None));
    p.stop();
    server.join().unwrap();
}
#[test]
fn disconnected_reports_are_dropped_and_start_stop_do_not_wait_for_broker() {
    let l = listener();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut p = Publisher::new(&cfg(port, "")).unwrap();
    assert!(!p.publish("/t", 1, None, None));
    let now = Instant::now();
    p.start().unwrap();
    assert!(now.elapsed() < Duration::from_secs(1));
    assert!(!p.publish("/t", 42, None, None));
    assert!(!p.publish("/t", 43, None, None));
    assert_eq!(p.dropped(), 3);
    p.stop();
    assert!(now.elapsed() < Duration::from_secs(1));
}
#[test]
fn timeout_and_invalid_topic_are_contained() {
    let l = listener();
    let mut p = Publisher::new(&cfg(l.local_addr().unwrap().port(), "  qos: 1\n")).unwrap();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, "/t", "42", 1, false, false);
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    wait(|| p.connected());
    assert!(!p.publish("bad/+", 1, None, None));
    let now = Instant::now();
    assert!(!p.publish("/t", 42, None, None));
    assert!(now.elapsed() >= Duration::from_millis(180));
    p.stop();
    server.join().unwrap();
}
#[test]
fn refusal_retries_and_resets_drop_counter() {
    let l = listener();
    let mut p = Publisher::new(&cfg(l.local_addr().unwrap().port(), "")).unwrap();
    let (refused_tx, refused_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        assert_eq!(packet(&mut s).0, 0x10);
        s.write_all(&[0x20, 2, 0, 5]).unwrap();
        drop(s);
        refused_tx.send(()).unwrap();
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    refused_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(!p.connected());
    assert!(!p.publish("/t", 1, None, None));
    assert_eq!(p.dropped(), 1);
    wait(|| p.connected());
    assert_eq!(p.dropped(), 0);
    assert_eq!(p.disconnected_for(), Duration::ZERO);
    p.stop();
    server.join().unwrap();
}
#[test]
fn connection_loss_reconnects_and_reannounces_online() {
    let l = listener();
    let mut p = Publisher::new(&cfg(l.local_addr().unwrap().port(), "")).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        drop(s);
        tx.send(()).unwrap();
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    wait(|| !p.connected());
    wait(|| p.connected());
    p.stop();
    server.join().unwrap();
}
#[test]
fn maintain_threshold_and_disabled_recreation() {
    for after in [0, 900] {
        let l = listener();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let clock = Arc::new(AtomicU64::new(1000));
        let c = clock.clone();
        let mut p = Publisher::with_clock(
            &cfg(port, &format!("  recreate_client_after_secs: {after}\n")),
            Arc::new(move || Duration::from_secs(c.load(Ordering::SeqCst))),
        )
        .unwrap();
        assert!(!p.maintain().unwrap());
        p.start().unwrap();
        clock.store(1899, Ordering::SeqCst);
        assert_eq!(p.disconnected_for(), Duration::from_secs(899));
        assert!(!p.maintain().unwrap());
        clock.store(1901, Ordering::SeqCst);
        assert_eq!(p.maintain().unwrap(), after != 0);
        if after != 0 {
            assert_eq!(p.disconnected_for(), Duration::ZERO);
        }
        p.stop();
    }
}
#[test]
fn invalid_tls_material_fails_without_network_or_secret_in_error() {
    let config = cfg(
        1883,
        "  tls: {enabled: true, ca_certs: /nonexistent/test-ca.pem}\n",
    );
    assert!(Publisher::new(&config).is_err());
    let config = cfg(1883, "  tls: {enabled: true}\n");
    assert!(Publisher::new(&config).is_ok());
}

#[test]
fn full_qos_queue_does_not_block_qos_zero() {
    let l = listener();
    let mut p = Publisher::new(&cfg(
        l.local_addr().unwrap().port(),
        "  max_queued_messages: 2\n",
    ))
    .unwrap();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        let connect = handshake(&mut s);
        assert_eq!(connect[7] & 0xc0, 0, "no username means no credentials");
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, "/first", "1", 1, false, false);
        publication(&mut s, "/second", "2", 1, false, false);
        publication(&mut s, "/live", "3", 0, false, false);
        // A full queue is temporary: a clean-session reconnect must free it.
        drop(s);
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, "/recovered", "1", 1, false, true);
        publication(&mut s, "/recovered", "2", 1, false, true);
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    wait(|| p.connected());
    assert!(!p.publish("/first", 1, Some(QoS::AtLeastOnce), None));
    assert!(!p.publish("/second", 2, Some(QoS::AtLeastOnce), None));
    let now = Instant::now();
    assert!(!p.publish("/rejected", 4, Some(QoS::AtLeastOnce), None));
    assert!(now.elapsed() < Duration::from_millis(180));
    assert!(p.publish("/live", 3, None, None));
    wait(|| !p.connected());
    wait(|| p.connected());
    // Barrier: the broker has acknowledged the online announcement before this.
    // Allow the worker to consume that acknowledgement before testing capacity.
    thread::sleep(Duration::from_millis(50));
    for value in 1..=2 {
        assert!(p.publish("/recovered", value, Some(QoS::AtLeastOnce), None));
    }
    p.stop();
    server.join().unwrap();
}
