use super::{ConfigError, fail};
use serde_yaml::Value;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) fn mode_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path).ok()?.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "{} is readable beyond its owner (mode {:04o}); chmod 600 it",
                path.display(),
                mode & 0o7777
            ));
        }
    }
    None
}

pub(super) fn resolve(
    mqtt: &Value,
    env: &HashMap<String, String>,
    warnings: &mut Vec<String>,
) -> Result<Option<String>, ConfigError> {
    if let Some(filename) = mqtt["password_file"].as_str() {
        let path = if filename == "~" || filename.starts_with("~/") {
            let home = env
                .get("HOME")
                .cloned()
                .or_else(|| std::env::var("HOME").ok())
                .ok_or_else(|| fail("mqtt.password_file", "cannot expand ~ without HOME"))?;
            PathBuf::from(home).join(filename.strip_prefix("~/").unwrap_or(""))
        } else {
            PathBuf::from(filename)
        };
        let secret = fs::read_to_string(&path).map_err(|e| {
            fail(
                "mqtt.password_file",
                &format!("cannot read {}: {e}", path.display()),
            )
        })?;
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(fail("mqtt.password_file", "file is empty"));
        }
        if let Some(warning) = mode_warning(&path) {
            warnings.push(warning);
        }
        return Ok(Some(secret.to_owned()));
    }
    if let Some(secret) = mqtt["password_env"]
        .as_str()
        .and_then(|key| env.get(key))
        .filter(|s| !s.is_empty())
    {
        return Ok(Some(secret.clone()));
    }
    if let Some(secret) = mqtt["password"].as_str() {
        warnings.push("mqtt.password is set inline in the config file; prefer mqtt.password_file or mqtt.password_env".into());
        return Ok(Some(secret.to_owned()));
    }
    Ok(None)
}
