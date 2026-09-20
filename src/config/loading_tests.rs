use super::*;
use std::fs;
#[test]
fn unchanged_example_config_loads_from_disk() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/config.yaml.example");
    let c = load(Some(&path), Some(&HashMap::new()), Some("myhost")).unwrap();
    assert_eq!(c["mqtt"]["host"].as_str(), Some("mqtt.example.lan"));
    assert_eq!(c.path.as_deref(), Some(path.as_path()));
    assert_eq!(c.meminfo_fields(), crate::consts::DEFAULT_MEMINFO_FIELDS);
    assert!(c.warnings.is_empty());
}
#[test]
fn explicitly_named_missing_config_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        load(
            Some(&dir.path().join("missing")),
            Some(&HashMap::new()),
            Some("host")
        )
        .is_err()
    );
}
#[test]
fn missing_default_config_uses_defaults_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let c = load_with_default(
        None,
        Some(&HashMap::new()),
        Some("myhost"),
        &dir.path().join("config.yaml"),
    )
    .unwrap();
    assert!(c.path.is_none());
    assert_eq!(c["mqtt"]["host"].as_str(), Some("localhost"));
    assert!(c.warnings[0].contains("using built-in defaults"));
}
#[test]
fn non_mapping_or_malformed_config_is_rejected() {
    let f = tempfile::NamedTempFile::new().unwrap();
    for yaml in ["just a string", "[1, 2]", "mqtt: [", "false", "0"] {
        fs::write(f.path(), yaml).unwrap();
        assert!(
            load(Some(f.path()), Some(&HashMap::new()), Some("host")).is_err(),
            "{yaml}"
        );
    }
}
#[test]
fn empty_and_null_documents_use_defaults() {
    for yaml in ["", "# comment only", "null", "---\n"] {
        let c = Config::from_yaml(yaml, "host", &HashMap::new()).unwrap();
        assert_eq!(c["mqtt"]["port"].as_u64(), Some(1883));
    }
}
#[test]
fn field_lists_replace_defaults_and_enable_flags_work() {
    let c = Config::from_yaml(
        "metrics: {meminfo: {fields: [MemAvailable]}, uptime_minutes: {enabled: false}}",
        "host",
        &HashMap::new(),
    )
    .unwrap();
    assert_eq!(c.meminfo_fields(), ["MemAvailable"]);
    assert!(c.meminfo_enabled());
    assert!(!c.uptime_enabled());
    let c = Config::from_yaml(
        "metrics: {meminfo: {enabled: false}}",
        "host",
        &HashMap::new(),
    )
    .unwrap();
    assert!(!c.meminfo_enabled());
    assert!(c.uptime_enabled());
}
#[test]
fn warning_callback_emits_each_warning() {
    let c = Config::from_yaml("mqtt: {hostname: typo}", "host", &HashMap::new()).unwrap();
    let mut logged = Vec::new();
    c.log_warnings(|w| logged.push(w.to_owned()));
    assert_eq!(
        logged,
        c.warnings
            .iter()
            .map(|w| format!("config: {w}"))
            .collect::<Vec<_>>()
    );
}
#[test]
fn load_without_hostname_uses_real_hostname() {
    let dir = tempfile::tempdir().unwrap();
    let c = load_with_default(
        None,
        Some(&HashMap::new()),
        None,
        &dir.path().join("config.yaml"),
    )
    .unwrap();
    assert_eq!(c.hostname, hostname::get().unwrap().to_string_lossy());
}
#[cfg(unix)]
#[test]
fn inline_password_checks_config_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = tempfile::NamedTempFile::new().unwrap();
    fs::write(f.path(), "mqtt: {password: synthetic-test-secret}").unwrap();
    fs::set_permissions(f.path(), fs::Permissions::from_mode(0o644)).unwrap();
    let c = load(Some(f.path()), Some(&HashMap::new()), Some("host")).unwrap();
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("readable beyond its owner"))
    );
}
