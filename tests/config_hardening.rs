use std::collections::HashMap;
use system_report::config::Config;

#[test]
fn malformed_or_invalid_secrets_are_not_echoed_in_errors() {
    for yaml in [
        "mqtt: {password: [synthetic-secret]}",
        "mqtt: {password: {synthetic-secret: value}}",
        "mqtt: {password: synthetic-secret",
    ] {
        let error = Config::from_yaml(yaml, "host", &HashMap::new())
            .err()
            .expect("must reject config");
        assert!(!error.to_string().contains("synthetic-secret"));
    }
}

#[test]
fn escaped_braces_and_replacement_values_are_not_expanded_recursively() {
    let c = Config::from_yaml(
        "topics: {prefix: '/{{literal}}/{hostname}'}",
        "{prefix}",
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(c.prefix, "/{literal}/{prefix}");
    assert_eq!(c.status_topic, "/{literal}/{prefix}/oper_state/status");
}

#[test]
fn home_for_password_files_uses_explicit_environment_first() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("pw"), "synthetic-secret\n").unwrap();
    let env = HashMap::from([("HOME".to_owned(), home.path().to_str().unwrap().to_owned())]);
    let c = Config::from_yaml("mqtt: {password_file: '~/pw'}", "host", &env).unwrap();
    assert_eq!(c.password.as_deref(), Some("synthetic-secret"));
    assert!(!format!("{:?}", c.redacted()).contains("synthetic-secret"));
}

#[test]
fn nullable_environment_and_tls_without_insecure_are_supported() {
    let env = HashMap::from([("SYSTEM_REPORT_MQTT_PASSWORD".into(), "ignored".into())]);
    let c = Config::from_yaml(
        "mqtt: {password_env: null, password: synthetic-secret, tls: {enabled: true}}",
        "host",
        &env,
    )
    .unwrap();
    assert_eq!(c.password.as_deref(), Some("synthetic-secret"));
    assert!(c.warnings.iter().any(|w| w.contains("inline")));
    assert!(
        !c.warnings
            .iter()
            .any(|w| w.contains("clear") || w.contains("NOT verified"))
    );
}

#[test]
fn yaml_merges_preserve_settings_and_redact_anchor_copies() {
    for yaml in [
        "mqtt: {<<: {host: broker.example.test, username: reporter, password: synthetic-secret}}",
        "defaults: &broker {host: broker.example.test, username: reporter, password: synthetic-secret}\nmqtt: {<<: *broker}",
        "mqtt: {<<: [{host: broker.example.test}, {host: ignored, password: synthetic-secret}], username: reporter}",
    ] {
        let c = Config::from_yaml(yaml, "host", &HashMap::new()).unwrap();
        assert_eq!(c["mqtt"]["host"].as_str(), Some("broker.example.test"));
        assert_eq!(c["mqtt"]["username"].as_str(), Some("reporter"));
        assert_eq!(c.password.as_deref(), Some("synthetic-secret"));
        assert!(
            !serde_yaml::to_string(&c.redacted())
                .unwrap()
                .contains("synthetic-secret")
        );
        assert!(c["mqtt"]["<<"].is_null());
    }
    let c = Config::from_yaml(
        "mqtt: {<<: {host: ignored}, host: explicit}",
        "host",
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(c["mqtt"]["host"].as_str(), Some("explicit"));
}

#[test]
fn invalid_yaml_merges_do_not_echo_secrets() {
    for yaml in [
        "mqtt: {<<: synthetic-secret}",
        "mqtt: {<<: [synthetic-secret]}",
    ] {
        let error = Config::from_yaml(yaml, "host", &HashMap::new())
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "cannot apply YAML merge keys");
    }
}
