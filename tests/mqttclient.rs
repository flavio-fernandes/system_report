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
    for value in 1..=2 {
        assert!(p.publish("/recovered", value, Some(QoS::AtLeastOnce), None));
    }
    p.stop();
    server.join().unwrap();
}

#[test]
fn connection_loss_fails_waiter_before_publish_timeout() {
    let l = listener();
    let config = Config::from_yaml(
        &format!(
            "mqtt:\n  host: 127.0.0.1\n  port: {}\n  qos: 1\n  publish_timeout_secs: 10\n",
            l.local_addr().unwrap().port()
        ),
        "testhost",
        &HashMap::new(),
    )
    .unwrap();
    let mut p = Publisher::new(&config).unwrap();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        publication(&mut s, "/unacked", "1", 1, false, false);
        // Drop the socket while publish() is still waiting for PubAck.
    });
    p.start().unwrap();
    wait(|| p.connected());
    let now = Instant::now();
    assert!(!p.publish("/unacked", 1, None, None));
    assert!(now.elapsed() < Duration::from_secs(2));
    p.stop();
    server.join().unwrap();
}

fn persistent_qos_two_reconnect(released: bool, session_present: bool) {
    let l = listener();
    let mut p = Publisher::new(&cfg(
        l.local_addr().unwrap().port(),
        "  clean_session: false\n",
    ))
    .unwrap();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        let (header, body) = packet(&mut s);
        assert_eq!(header, 0x34);
        let mut offset = 0;
        assert_eq!(field(&body, &mut offset), b"/old");
        let id = [body[offset], body[offset + 1]];
        if released {
            s.write_all(&[0x50, 2, id[0], id[1]]).unwrap();
            assert_eq!(packet(&mut s), (0x62, id.to_vec()));
        }
        drop(s);
        let mut s = accept(&l);
        assert_eq!(packet(&mut s).0, 0x10);
        s.write_all(&[0x20, 2, u8::from(session_present), 0])
            .unwrap();
        if session_present {
            if !released {
                let (header, replay) = packet(&mut s);
                assert_eq!(header & !8, 0x34);
                assert_eq!(replay, body, "resumed PUBLISH keeps payload and packet ID");
                s.write_all(&[0x50, 2, id[0], id[1]]).unwrap();
                // Online may be emitted before the broker's PUBREC is processed.
                // Consume it below while waiting for the old PUBREL.
            }
            let mut online = false;
            loop {
                let (header, b) = packet(&mut s);
                if header == 0x62 {
                    assert_eq!(b, id);
                    // Keep the old exchange unfinished while a new one starts.
                    break;
                }
                assert_eq!(header, 0x33);
                let mut offset = 0;
                assert_eq!(field(&b, &mut offset), STATUS.as_bytes());
                s.write_all(&[0x40, 2, b[offset], b[offset + 1]]).unwrap();
                online = true;
            }
            if !online {
                publication(&mut s, STATUS, "online", 1, true, true);
            }
        } else {
            publication(&mut s, STATUS, "online", 1, true, true);
        }
        let (header, body) = packet(&mut s);
        assert_eq!(header, 0x34);
        let mut offset = 0;
        assert_eq!(field(&body, &mut offset), b"/new");
        let new_id = [body[offset], body[offset + 1]];
        if session_present {
            assert_ne!(
                new_id, id,
                "must not reuse an unfinished exchange's packet ID"
            );
        }
        s.write_all(&[0x50, 2, new_id[0], new_id[1]]).unwrap();
        assert_eq!(packet(&mut s), (0x62, new_id.to_vec()));
        s.write_all(&[0x70, 2, new_id[0], new_id[1]]).unwrap();
        if session_present {
            s.write_all(&[0x70, 2, id[0], id[1]]).unwrap();
        }
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    wait(|| p.connected());
    assert!(!p.publish("/old", "old", Some(QoS::ExactlyOnce), None));
    wait(|| !p.connected());
    wait(|| p.connected());
    assert!(p.publish("/new", "new", Some(QoS::ExactlyOnce), None));
    p.stop();
    server.join().unwrap();
}

#[test]
fn persistent_session_replays_unacknowledged_publish() {
    persistent_qos_two_reconnect(false, true);
}
#[test]
fn persistent_session_resumes_pubrel() {
    persistent_qos_two_reconnect(true, true);
}
#[test]
fn absent_broker_session_clears_old_waiters() {
    persistent_qos_two_reconnect(false, false);
}

#[test]
fn persistent_session_is_not_discarded_by_outage_recreation() {
    let clock = Arc::new(AtomicU64::new(0));
    let c = clock.clone();
    let l = listener();
    let port = l.local_addr().unwrap().port();
    drop(l);
    let mut p = Publisher::with_clock(
        &cfg(
            port,
            "  clean_session: false\n  recreate_client_after_secs: 1\n",
        ),
        Arc::new(move || Duration::from_secs(c.load(Ordering::SeqCst))),
    )
    .unwrap();
    p.start().unwrap();
    clock.store(1000, Ordering::SeqCst);
    assert!(!p.maintain().unwrap());
    p.stop();
}

#[test]
fn qos_zero_waits_for_wire_progress_at_underlying_inflight_limit() {
    let l = listener();
    let mut p = Publisher::new(&cfg(
        l.local_addr().unwrap().port(),
        "  max_queued_messages: 0\n",
    ))
    .unwrap();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (delivered_tx, delivered_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let mut s = accept(&l);
        handshake(&mut s);
        publication(&mut s, STATUS, "online", 1, true, true);
        let mut ids = Vec::new();
        for _ in 0..100 {
            let (header, body) = packet(&mut s);
            assert_eq!(header, 0x32);
            let mut offset = 0;
            assert_eq!(field(&body, &mut offset), b"/unacked");
            ids.push([body[offset], body[offset + 1]]);
        }
        // The caller must time out, rather than report a queued packet as sent.
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        for id in ids {
            s.write_all(&[0x40, 2, id[0], id[1]]).unwrap();
        }
        publication(&mut s, "/live", "value", 0, false, false);
        delivered_tx.send(()).unwrap();
        publication(&mut s, "/after", "value", 0, false, false);
        publication(&mut s, STATUS, "offline", 1, true, true);
        assert_eq!(packet(&mut s).0, 0xe0);
    });
    p.start().unwrap();
    wait(|| p.connected());
    for _ in 0..100 {
        assert!(!p.publish("/unacked", "value", Some(QoS::AtLeastOnce), None));
    }
    assert!(!p.publish("/live", "value", None, None));
    release_tx.send(()).unwrap();
    delivered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(p.publish("/after", "value", None, None));
    p.stop();
    server.join().unwrap();
}

// Exercise the production Linux/OpenSSL native-tls backend with disposable keys.
#[cfg(target_os = "linux")]
#[test]
fn tls_identity_accepts_separate_and_combined_pkcs8_files() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("client.key");
    let cert = dir.path().join("client.crt");
    let output = std::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-days",
            "1",
            "-subj",
            "/CN=loopback-test",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&cert)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "disposable TLS fixture generation failed"
    );
    let config = cfg(
        1883,
        &format!(
            "  tls: {{enabled: true, certfile: '{}', keyfile: '{}'}}\n",
            cert.display(),
            key.display()
        ),
    );
    assert!(Publisher::new(&config).is_ok());
    let c = std::fs::read(&cert).unwrap();
    let k = std::fs::read(&key).unwrap();
    let combined = dir.path().join("combined.pem");
    for bytes in [
        [c.as_slice(), k.as_slice()].concat(),
        [k.as_slice(), c.as_slice()].concat(),
    ] {
        std::fs::write(&combined, bytes).unwrap();
        let config = cfg(
            1883,
            &format!(
                "  tls: {{enabled: true, certfile: '{}'}}\n",
                combined.display()
            ),
        );
        assert!(Publisher::new(&config).is_ok());
    }
    let legacy = dir.path().join("legacy.key");
    let output = std::process::Command::new("openssl")
        .args(["rsa", "-traditional", "-in"])
        .arg(&key)
        .arg("-out")
        .arg(&legacy)
        .output()
        .unwrap();
    assert!(output.status.success());
    let config = cfg(
        1883,
        &format!(
            "  tls: {{enabled: true, certfile: '{}', keyfile: '{}'}}\n",
            cert.display(),
            legacy.display()
        ),
    );
    let error = Publisher::new(&config).err().unwrap().to_string();
    assert!(error.contains("PKCS#8"));
    assert!(!error.contains("BEGIN"));
}
