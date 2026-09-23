//! Frozen oracle generated from the original const.py with Python's runpy/json.
//! No Python interpreter is needed to build or test the Rust port.
use serde_yaml::{Value, to_value};
use std::collections::HashMap;
use system_report::{config::Config, consts::*};

#[test]
fn all_defaults_match_python() {
    let oracle: Value =
        serde_yaml::from_str(include_str!("fixtures/python_constants.json")).unwrap();
    let defaults: Value = serde_yaml::from_str(DEFAULTS_YAML).unwrap();
    assert_eq!(defaults, oracle["DEFAULTS"]);
    assert_eq!(
        to_value(DEFAULT_MEMINFO_FIELDS).unwrap(),
        oracle["DEFAULT_MEMINFO_FIELDS"]
    );
    let names: std::collections::BTreeMap<_, _> = MEMINFO_TOPIC_NAMES.iter().copied().collect();
    assert_eq!(to_value(names).unwrap(), oracle["MEMINFO_TOPIC_NAMES"]);
    for (key, value) in [
        ("VERSION", VERSION),
        ("APP_NAME", APP_NAME),
        ("PROC_UPTIME", PROC_UPTIME),
        ("PROC_MEMINFO", PROC_MEMINFO),
    ] {
        assert_eq!(oracle[key].as_str(), Some(value));
    }
    for (key, value) in [
        ("LOOP_TICK_MAX_SECS", LOOP_TICK_MAX_SECS),
        ("PENDING_RETRY_SECS", PENDING_RETRY_SECS),
        ("DROP_LOG_INTERVAL_SECS", DROP_LOG_INTERVAL_SECS),
    ] {
        assert_eq!(oracle[key].as_f64(), Some(value));
    }
    assert_eq!(
        oracle["MAX_CONSECUTIVE_CYCLE_FAILURES"].as_u64(),
        Some(MAX_CONSECUTIVE_CYCLE_FAILURES.into())
    );
}

#[test]
fn example_differs_from_defaults_only_in_broker_hostname() {
    let c = Config::from_yaml(
        include_str!("../data/config.yaml.example"),
        "host",
        &HashMap::new(),
    )
    .unwrap();
    let mut data = c.redacted();
    data["mqtt"]["host"] = Value::String("localhost".into());
    assert_eq!(data, serde_yaml::from_str::<Value>(DEFAULTS_YAML).unwrap());
}

#[test]
fn all_known_topic_names_and_fallback_match() {
    for (field, topic) in MEMINFO_TOPIC_NAMES {
        assert_eq!(meminfo_topic_name(field), *topic);
    }
    assert_eq!(meminfo_topic_name("HugePages_Total"), "huge_pages_total_kb");
    assert_eq!(meminfo_topic_name("Foo__Bar"), "foo_bar_kb");
    assert_eq!(meminfo_topic_name("ABC"), "abc_kb");
}
