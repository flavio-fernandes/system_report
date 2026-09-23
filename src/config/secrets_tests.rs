use super::*;
use std::{fs, os::unix::fs::PermissionsExt};
fn config(yaml: &str, env: &[(&str, &str)]) -> Config {
    Config::from_yaml(
        yaml,
        "myhost",
        &env.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
    .unwrap()
}
fn secret_file(content: &str, mode: u32) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    fs::write(file.path(), content).unwrap();
    fs::set_permissions(file.path(), fs::Permissions::from_mode(mode)).unwrap();
    file
}
#[test]
fn password_file_wins_and_is_stripped() {
    let file = secret_file("s3cret\n", 0o600);
    let c = config(
        &format!(
            "mqtt: {{password_file: '{}', password: inline}}",
            file.path().display()
        ),
        &[("SYSTEM_REPORT_MQTT_PASSWORD", "from-env")],
    );
    assert_eq!(c.password.as_deref(), Some("s3cret"));
    assert!(!c.warnings.iter().any(|w| w.contains("readable beyond")));
    assert!(!c.warnings.iter().any(|w| w.contains("inline")));
}
#[test]
fn world_readable_password_file_is_flagged() {
    let file = secret_file("s3cret", 0o644);
    let c = config(
        &format!("mqtt: {{password_file: '{}'}}", file.path().display()),
        &[],
    );
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("readable beyond its owner"))
    );
}
#[test]
fn empty_or_missing_password_file_fails_without_fallback() {
    let file = secret_file("\n", 0o600);
    for path in [
        file.path().to_path_buf(),
        file.path().with_extension("missing"),
    ] {
        assert!(
            Config::from_yaml(
                &format!(
                    "mqtt: {{password_file: '{}', password: fallback}}",
                    path.display()
                ),
                "host",
                &HashMap::new()
            )
            .is_err()
        );
    }
}
#[test]
fn environment_beats_inline_and_empty_environment_falls_back() {
    let c = config(
        "mqtt: {password: inline}",
        &[("SYSTEM_REPORT_MQTT_PASSWORD", "from-env")],
    );
    assert_eq!(c.password.as_deref(), Some("from-env"));
    let c = config(
        "mqtt: {password: inline}",
        &[("SYSTEM_REPORT_MQTT_PASSWORD", "")],
    );
    assert_eq!(c.password.as_deref(), Some("inline"));
}
#[test]
fn inline_password_warns_and_is_redacted_without_mutating_live_secret() {
    let c = config("mqtt: {username: bob, password: s3cret}", &[]);
    assert_eq!(c.password.as_deref(), Some("s3cret"));
    assert!(c.warnings.iter().any(|w| w.contains("inline")));
    assert!(c.warnings.iter().any(|w| w.contains("clear")));
    assert_eq!(c.redacted()["mqtt"]["password"].as_str(), Some(REDACTED));
    assert!(!format!("{:?}", c.redacted()).contains("s3cret"));
    assert_eq!(c["mqtt"]["password"].as_str(), Some("s3cret"));
}
#[test]
fn username_without_password_warns() {
    let c = config("mqtt: {username: bob}", &[]);
    assert!(c.password.is_none());
    assert!(c.warnings.iter().any(|w| w.contains("no password")));
}
#[test]
fn insecure_tls_warns() {
    let c = config("mqtt: {tls: {enabled: true, insecure: true}}", &[]);
    assert!(c.warnings.iter().any(|w| w.contains("NOT verified")));
}
#[test]
fn unknown_keys_are_kept_but_warned_about() {
    let c = config("mqtt: {hostname: typo.example.lan}", &[]);
    assert_eq!(c["mqtt"]["hostname"].as_str(), Some("typo.example.lan"));
    assert!(c.warnings.iter().any(|w| w.contains("mqtt.hostname")));
}
