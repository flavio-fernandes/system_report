//! Recursive merge and startup validation. Unknown keys are preserved for diagnostics.
use serde_yaml::Value;
use std::path::{Path, PathBuf};
use std::{collections::HashMap, fmt};
mod secrets;
pub const CFG_FILENAME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/data/config.yaml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}
fn fail(key: &str, message: &str) -> ConfigError {
    ConfigError(format!("config key '{key}': {message}"))
}

pub const REDACTED: &str = "<redacted>";
pub struct Config {
    data: Value,
    pub path: Option<PathBuf>,
    pub password: Option<String>,
    pub warnings: Vec<String>,
    pub hostname: String,
    pub prefix: String,
    pub status_topic: String,
    pub json_topic: String,
    pub uptime_topic: String,
}
impl std::ops::Index<&str> for Config {
    type Output = Value;
    fn index(&self, key: &str) -> &Value {
        &self.data[key]
    }
}
fn deep_merge(base: &mut Value, overlay: Value, path: &str, warnings: &mut Vec<String>) {
    if let (Some(base), Some(overlay)) = (base.as_mapping_mut(), overlay.as_mapping()) {
        for (key, value) in overlay {
            let name = key.as_str().unwrap_or("<non-string key>");
            let where_ = if path.is_empty() {
                name.to_owned()
            } else {
                format!("{path}.{name}")
            };
            if let Some(old) = base.get_mut(key) {
                deep_merge(old, value.clone(), &where_, warnings);
            } else {
                warnings.push(format!(
                    "unknown config key '{where_}' (ignored, check for a typo)"
                ));
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = overlay;
    }
}
// Anchored defaults can remain under an unknown key after a YAML merge.
// Redact their password fields too, including nested diagnostic values.
fn redact_passwords(value: &mut Value) {
    match value {
        Value::Mapping(entries) => {
            for (key, value) in entries {
                if key.as_str() == Some("password") && !value.is_null() {
                    *value = Value::String(REDACTED.into());
                } else {
                    redact_passwords(value);
                }
            }
        }
        Value::Sequence(values) => values.iter_mut().for_each(redact_passwords),
        Value::Tagged(value) => redact_passwords(&mut value.value),
        _ => {}
    }
}
fn dig<'a>(data: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(data, |node, part| &node[part])
}
// Derive the basic shape from the defaults, so every knob is type checked.
// Errors deliberately omit values: a malformed secret must not reach logs.
fn validate_shape(value: &Value, default: &Value, path: &str) -> Result<(), ConfigError> {
    match default {
        Value::Mapping(entries) => {
            if !value.is_mapping() {
                return Err(fail(path, "expected a mapping"));
            }
            for (key, default) in entries {
                let key = key.as_str().expect("built-in key");
                let where_ = if path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{path}.{key}")
                };
                validate_shape(&value[key], default, &where_)?;
            }
        }
        Value::Null => {
            if !value.is_null() && value.as_str().is_none_or(|s| s.trim().is_empty()) {
                return Err(fail(path, "expected a non-empty string or null"));
            }
        }
        Value::Bool(_) => {
            if !value.is_bool() {
                return Err(fail(path, "expected true/false"));
            }
        }
        Value::Number(n) if n.is_f64() => {
            if !value.as_f64().is_some_and(|n| n.is_finite() && n >= 0.1) {
                return Err(fail(path, "expected a finite number >= 0.1"));
            }
        }
        Value::Number(_) => {
            let (min, max) = match path {
                "mqtt.port" => (1, u64::from(u16::MAX)),
                "mqtt.keepalive_secs" => (5, u64::from(u16::MAX)),
                "mqtt.qos" => (0, 2),
                "mqtt.recreate_client_after_secs" | "mqtt.max_queued_messages" => (0, u64::MAX),
                _ => (1, u64::MAX),
            };
            if !value.as_u64().is_some_and(|n| n >= min && n <= max) {
                return Err(fail(path, &format!("expected an integer in {min}..={max}")));
            }
        }
        Value::String(_) => {
            // password_env is a nullable string despite its non-null default.
            if path == "mqtt.password_env" && value.is_null() {
                return Ok(());
            }
            if !value
                .as_str()
                .is_some_and(|s| path == "topics.prefix" || !s.trim().is_empty())
            {
                return Err(fail(path, "expected a non-empty string"));
            }
        }
        Value::Sequence(_) => {
            if !value.as_sequence().is_some_and(|s| {
                !s.is_empty()
                    && s.iter()
                        .all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
            }) {
                return Err(fail(
                    path,
                    "expected a non-empty list of non-empty field names",
                ));
            }
        }
        _ => unreachable!("built-in defaults contain no YAML tags"),
    }
    Ok(())
}
fn check_topic(topic: &str, key: &str) -> Result<(), ConfigError> {
    if topic.is_empty() {
        return Err(fail(key, "resolved to an empty topic"));
    }
    if topic.contains(['+', '#', '\0']) {
        return Err(fail(key, "publish topics cannot contain +, # or NUL"));
    }
    Ok(())
}
fn validate(data: &Value, defaults: &Value) -> Result<(), ConfigError> {
    validate_shape(data, defaults, "")?;
    if data["mqtt"]["reconnect_max_delay_secs"].as_u64()
        < data["mqtt"]["reconnect_min_delay_secs"].as_u64()
    {
        return Err(fail(
            "mqtt.reconnect_max_delay_secs",
            "must be >= mqtt.reconnect_min_delay_secs",
        ));
    }
    for key in [
        "topics.status",
        "topics.json",
        "metrics.uptime_minutes.topic",
        "metrics.meminfo.topic",
    ] {
        check_topic(dig(data, key).as_str().unwrap(), key)?;
    }
    if !dig(data, "metrics.meminfo.topic")
        .as_str()
        .unwrap()
        .contains("{name}")
    {
        return Err(fail(
            "metrics.meminfo.topic",
            "must contain {name} so each field gets its own topic",
        ));
    }
    for (section, first, second) in [
        (
            "metrics",
            "metrics.uptime_minutes.enabled",
            "metrics.meminfo.enabled",
        ),
        ("report", "report.publish_plain", "report.publish_json"),
    ] {
        if dig(data, first).as_bool() == Some(false) && dig(data, second).as_bool() == Some(false) {
            return Err(fail(
                section,
                "both outputs are disabled; nothing would be reported",
            ));
        }
    }
    Ok(())
}
impl Config {
    pub fn meminfo_fields(&self) -> Vec<String> {
        self.data["metrics"]["meminfo"]["fields"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }
    pub fn uptime_enabled(&self) -> bool {
        self.data["metrics"]["uptime_minutes"]["enabled"]
            .as_bool()
            .unwrap()
    }
    pub fn meminfo_enabled(&self) -> bool {
        self.data["metrics"]["meminfo"]["enabled"]
            .as_bool()
            .unwrap()
    }
    /// The logging layer supplies a closure; configuration does not own the logger.
    pub fn log_warnings(&self, mut logger: impl FnMut(&str)) {
        for warning in &self.warnings {
            logger(&format!("config: {warning}"));
        }
    }
    pub fn redacted(&self) -> Value {
        let mut data = self.data.clone();
        redact_passwords(&mut data);
        data
    }
    pub fn client_id(&self) -> String {
        self.data["mqtt"]["client_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{}_{}", crate::consts::APP_NAME, self.hostname))
    }
    pub fn meminfo_topic(&self, field: &str) -> Result<String, ConfigError> {
        let key = "metrics.meminfo.topic";
        let topic = format_template(
            dig(&self.data, key).as_str().unwrap(),
            key,
            &self.hostname,
            &self.prefix,
            Some(&crate::consts::meminfo_topic_name(field)),
        )?;
        check_topic(&topic, key)?;
        Ok(topic)
    }
    pub fn from_yaml(
        yaml: &str,
        hostname: &str,
        env: &HashMap<String, String>,
    ) -> Result<Self, ConfigError> {
        let defaults: Value =
            serde_yaml::from_str(crate::consts::DEFAULTS_YAML).expect("valid built-in defaults");
        let mut data = defaults.clone();
        // Do not include serde's error text: it can echo an inline password.
        let mut raw: Value = serde_yaml::from_str(yaml).map_err(|e| {
            let location = e
                .location()
                .map(|l| format!(" at line {}, column {}", l.line(), l.column()))
                .unwrap_or_default();
            ConfigError(format!("cannot parse YAML{location}"))
        })?;
        if !raw.is_mapping() && !raw.is_null() {
            return Err(ConfigError("expected a mapping at the top level".into()));
        }
        raw.apply_merge()
            .map_err(|_| ConfigError("cannot apply YAML merge keys".into()))?;
        let mut warnings = Vec::new();
        if !raw.is_null() {
            deep_merge(&mut data, raw, "", &mut warnings);
        }
        validate(&data, &defaults)?;
        let prefix = format_template(
            data["topics"]["prefix"].as_str().unwrap(),
            "topics.prefix",
            hostname,
            "",
            None,
        )?
        .trim_end_matches('/')
        .to_owned();
        let topic = |key: &str| -> Result<String, ConfigError> {
            let topic = format_template(
                dig(&data, key).as_str().unwrap(),
                key,
                hostname,
                &prefix,
                None,
            )?;
            check_topic(&topic, key)?;
            Ok(topic)
        };
        let status_topic = topic("topics.status")?;
        let json_topic = topic("topics.json")?;
        let uptime_topic = topic("metrics.uptime_minutes.topic")?;
        let password = secrets::resolve(&data["mqtt"], env, &mut warnings)?;
        if data["mqtt"]["username"].as_str().is_some() && password.is_none() {
            warnings.push(
                "mqtt.username is set but no password was found; connecting without one".into(),
            );
        }
        let tls = data["mqtt"]["tls"]["enabled"].as_bool().unwrap();
        if tls && data["mqtt"]["tls"]["insecure"].as_bool().unwrap() {
            warnings
                .push("mqtt.tls.insecure is true: the broker certificate is NOT verified".into());
        }
        if !tls && password.is_some() {
            warnings.push(
                "TLS is disabled, so the MQTT password crosses the network in the clear".into(),
            );
        }
        let cfg = Self {
            data,
            path: None,
            password,
            warnings,
            hostname: hostname.to_owned(),
            prefix,
            status_topic,
            json_topic,
            uptime_topic,
        };
        // Validate every meminfo template at startup, not at the first report.
        for field in cfg.data["metrics"]["meminfo"]["fields"]
            .as_sequence()
            .unwrap()
        {
            cfg.meminfo_topic(field.as_str().unwrap())?;
        }
        Ok(cfg)
    }
}
#[cfg(test)]
mod loading_tests;
#[cfg(test)]
mod secrets_tests;
#[cfg(test)]
mod tests;

/// Load an explicit file, or the checkout's data/config.yaml. Only the default
/// path may be absent. Pass an explicit path when running an installed binary.
/// `Some(empty_map)` deliberately disables process-environment secret lookup.
pub fn load(
    path: Option<&Path>,
    env: Option<&HashMap<String, String>>,
    hostname: Option<&str>,
) -> Result<Config, ConfigError> {
    load_with_default(path, env, hostname, Path::new(CFG_FILENAME))
}

fn load_with_default(
    path: Option<&Path>,
    env: Option<&HashMap<String, String>>,
    hostname: Option<&str>,
    default_path: &Path,
) -> Result<Config, ConfigError> {
    let path = path.unwrap_or(default_path);
    let (yaml, exists) = match std::fs::read_to_string(path) {
        Ok(yaml) => (yaml, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && path == default_path => {
            (String::new(), false)
        }
        Err(e) => {
            return Err(ConfigError(format!(
                "cannot read config {}: {e}",
                path.display()
            )));
        }
    };
    let system_env;
    let env = match env {
        Some(env) => env,
        None => {
            system_env = std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect();
            &system_env
        }
    };
    let system_hostname;
    let hostname = match hostname {
        Some(hostname) => hostname,
        None => {
            system_hostname = hostname::get()
                .map_err(|e| ConfigError(format!("cannot get hostname: {e}")))?
                .to_string_lossy()
                .into_owned();
            &system_hostname
        }
    };
    let mut cfg = Config::from_yaml(&yaml, hostname, env)
        .map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
    if exists {
        cfg.path = Some(path.to_owned());
        if cfg
            .warnings
            .iter()
            .any(|w| w.starts_with("mqtt.password is set inline"))
            && let Some(warning) = secrets::mode_warning(path)
        {
            cfg.warnings.push(warning);
        }
    } else {
        cfg.warnings.insert(
            0,
            format!(
                "no config file at {}; using built-in defaults (broker {}:{})",
                path.display(),
                cfg["mqtt"]["host"].as_str().unwrap(),
                cfg["mqtt"]["port"].as_u64().unwrap()
            ),
        );
    }
    Ok(cfg)
}

/// The documented placeholders plus Python-style escaped braces. No recursive expansion.
fn format_template(
    template: &str,
    key: &str,
    hostname: &str,
    prefix: &str,
    name: Option<&str>,
) -> Result<String, ConfigError> {
    let mut result = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                result.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                result.push('}');
            }
            '{' => {
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(c) => field.push(c),
                        None => return Err(fail(key, "unclosed placeholder")),
                    }
                }
                let value = match field.as_str() {
                    "hostname" => hostname,
                    "prefix" => prefix,
                    "name" => {
                        name.ok_or_else(|| fail(key, "name is only valid in a meminfo topic"))?
                    }
                    _ => {
                        return Err(fail(
                            key,
                            "unknown placeholder (expected hostname, prefix or name)",
                        ));
                    }
                };
                result.push_str(value);
            }
            '}' => return Err(fail(key, "unmatched closing brace")),
            _ => result.push(c),
        }
    }
    Ok(result)
}
