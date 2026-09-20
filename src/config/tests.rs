use super::*;
#[test]
fn topics_and_client_id_expand_hostname_and_prefix() {
    let c = cfg("{}");
    assert_eq!(c.prefix, "/myhost");
    assert_eq!(c.status_topic, "/myhost/oper_state/status");
    assert_eq!(c.json_topic, "/myhost/oper_state/json");
    assert_eq!(c.uptime_topic, "/myhost/oper_uptime_minutes");
    assert_eq!(
        c.meminfo_topic("MemAvailable").unwrap(),
        "/myhost/oper_state/mem_available_kb"
    );
    assert_eq!(
        c.meminfo_topic("SUnreclaim").unwrap(),
        "/myhost/oper_state/sunreclaim_kb"
    );
    assert_eq!(
        c.meminfo_topic("HugePages_Total").unwrap(),
        "/myhost/oper_state/huge_pages_total_kb"
    );
    assert_eq!(c.client_id(), "system_report_myhost");
    let c = cfg("topics: {prefix: '/sensors/box1/'}\nmqtt: {client_id: box1}");
    assert_eq!(c.prefix, "/sensors/box1");
    assert_eq!(c.status_topic, "/sensors/box1/oper_state/status");
    assert_eq!(c.client_id(), "box1");
}
#[test]
fn bad_templates_fail_at_startup() {
    for yaml in [
        "topics: {prefix: '/{nope}'}",
        "topics: {status: '{'}",
        "metrics: {meminfo: {topic: '{prefix}/{name}/{nope}'}}",
        "topics: {prefix: '/bad/#'}",
        "topics: {prefix: '', status: '{prefix}'}",
    ] {
        assert!(
            Config::from_yaml(yaml, "host", &HashMap::new()).is_err(),
            "{yaml}"
        );
    }
}

#[test]
fn rejects_bad_values_with_key_named() {
    for (yaml, key) in [
        ("mqtt: {port: 0}", "mqtt.port"),
        ("mqtt: {port: '1883'}", "mqtt.port"),
        ("mqtt: {host: ''}", "mqtt.host"),
        ("mqtt: {qos: 3}", "mqtt.qos"),
        ("mqtt: {retain: 'yes'}", "mqtt.retain"),
        ("mqtt: {keepalive_secs: 1}", "mqtt.keepalive_secs"),
        (
            "mqtt: {reconnect_min_delay_secs: 60, reconnect_max_delay_secs: 30}",
            "mqtt.reconnect_max_delay_secs",
        ),
        (
            "mqtt: {publish_timeout_secs: 0}",
            "mqtt.publish_timeout_secs",
        ),
        ("report: {interval_secs: 0}", "report.interval_secs"),
        (
            "report: {publish_plain: false, publish_json: false}",
            "report",
        ),
        ("metrics: {meminfo: {fields: []}}", "metrics.meminfo.fields"),
        (
            "metrics: {meminfo: {fields: MemAvailable}}",
            "metrics.meminfo.fields",
        ),
        (
            "metrics: {meminfo: {topic: '{prefix}/mem'}}",
            "metrics.meminfo.topic",
        ),
        ("topics: {status: '{prefix}/#'}", "topics.status"),
        ("topics: {json: '{prefix}/+/json'}", "topics.json"),
        (
            "metrics: {meminfo: {enabled: false}, uptime_minutes: {enabled: false}}",
            "metrics",
        ),
        ("mqtt: null", "mqtt"),
        ("mqtt: {tls: []}", "mqtt.tls"),
        ("mqtt: {port: true}", "mqtt.port"),
        ("mqtt: {port: 65536}", "mqtt.port"),
        (
            "mqtt: {publish_timeout_secs: .nan}",
            "mqtt.publish_timeout_secs",
        ),
        (
            "metrics: {meminfo: {fields: ['']}}",
            "metrics.meminfo.fields",
        ),
    ] {
        let error = Config::from_yaml(yaml, "myhost", &HashMap::new())
            .err()
            .unwrap_or_else(|| panic!("accepted invalid config: {yaml}"));
        assert!(error.to_string().contains(key), "{key}: {error}");
    }
}

fn cfg(yaml: &str) -> Config {
    Config::from_yaml(yaml, "myhost", &HashMap::new()).unwrap()
}
#[test]
fn defaults_apply_and_overrides_win() {
    let c = cfg("mqtt: {host: broker.example.lan}\nreport: {interval_secs: 30}");
    assert_eq!(c["mqtt"]["host"].as_str(), Some("broker.example.lan"));
    assert_eq!(c["mqtt"]["port"].as_u64(), Some(1883));
    assert_eq!(c["report"]["interval_secs"].as_u64(), Some(30));
    assert_eq!(c["report"]["publish_json"].as_bool(), Some(true));
}
