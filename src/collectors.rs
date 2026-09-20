//! Linux proc collectors, with injectable readers for deterministic tests.
pub use crate::consts::meminfo_topic_name;

pub fn parse_uptime_seconds(text: &str) -> Result<f64, String> {
    let field = text
        .split_whitespace()
        .next()
        .ok_or("missing uptime seconds")?;
    let seconds: f64 = field.parse().map_err(|_| "invalid uptime seconds")?;
    // JSON integer conversion must not silently saturate or turn NaN into zero.
    if !seconds.is_finite() || seconds < 0.0 || seconds >= u64::MAX as f64 {
        return Err("uptime seconds must be finite, nonnegative and fit in u64".into());
    }
    Ok(seconds)
}
pub fn parse_uptime_minutes(text: &str) -> Result<u64, String> {
    Ok((parse_uptime_seconds(text)? / 60.0) as u64)
}

/// Ordered field/value pairs in raw kernel units (normally kB).
pub type Meminfo = Vec<(String, i64)>;
pub fn parse_meminfo(text: &str, fields: Option<&[&str]>) -> Meminfo {
    let mut parsed = std::collections::BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(key), Some(value)) = (parts.next(), parts.next())
            && let Some(key) = key.strip_suffix(':')
            && let Ok(value) = value.parse::<i64>()
        {
            parsed.insert(key.to_owned(), value);
        }
    }
    match fields {
        None => parsed.into_iter().collect(),
        Some(fields) => fields
            .iter()
            .filter_map(|field| {
                parsed
                    .remove(*field)
                    .map(|value| ((*field).to_owned(), value))
            })
            .collect(),
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub ts: chrono::DateTime<chrono::Utc>,
    pub hostname: Option<String>,
    pub uptime_s: Option<f64>,
    pub uptime_minutes: Option<u64>,
    pub meminfo: Meminfo,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CollectOptions<'a> {
    pub fields: &'a [&'a str],
    pub want_uptime: bool,
    pub want_meminfo: bool,
    pub hostname: Option<&'a str>,
    pub now: Option<chrono::DateTime<chrono::Utc>>,
}
impl Default for CollectOptions<'_> {
    fn default() -> Self {
        Self {
            fields: crate::consts::DEFAULT_MEMINFO_FIELDS,
            want_uptime: true,
            want_meminfo: true,
            hostname: None,
            now: None,
        }
    }
}
/// Read real proc files. Unsupported/missing proc files become per-metric errors.
pub fn collect(options: &CollectOptions<'_>) -> Snapshot {
    collect_with_reader(options, |path| std::fs::read_to_string(path))
}

/// Reader failures never suppress the other collector. Reader callbacks must not panic.
pub fn collect_with_reader<F>(options: &CollectOptions<'_>, mut reader: F) -> Snapshot
where
    F: FnMut(&str) -> std::io::Result<String>,
{
    use crate::consts::{PROC_MEMINFO, PROC_UPTIME};
    let mut snapshot = Snapshot {
        ts: options.now.unwrap_or_else(chrono::Utc::now),
        hostname: None,
        uptime_s: None,
        uptime_minutes: None,
        meminfo: Vec::new(),
        errors: Vec::new(),
    };
    if options.want_uptime {
        match reader(PROC_UPTIME)
            .map_err(|e| e.to_string())
            .and_then(|text| parse_uptime_seconds(&text))
        {
            Ok(seconds) => {
                snapshot.uptime_s = Some(seconds);
                snapshot.uptime_minutes = Some((seconds / 60.0) as u64);
            }
            Err(error) => snapshot.errors.push(format!("{PROC_UPTIME}: {error}")),
        }
    }
    if options.want_meminfo {
        match reader(PROC_MEMINFO) {
            Ok(text) => {
                snapshot.meminfo = parse_meminfo(&text, Some(options.fields));
                let missing: Vec<_> = options
                    .fields
                    .iter()
                    .copied()
                    .filter(|field| !snapshot.meminfo.iter().any(|(key, _)| key == field))
                    .collect();
                if !missing.is_empty() {
                    snapshot.errors.push(format!(
                        "{PROC_MEMINFO}: fields not present in this kernel: {}",
                        missing.join(", ")
                    ));
                }
            }
            Err(error) => snapshot.errors.push(format!("{PROC_MEMINFO}: {error}")),
        }
    }
    snapshot.hostname = match options.hostname.filter(|name| !name.is_empty()) {
        Some(name) => Some(name.into()),
        None => match hostname::get().and_then(|name| {
            name.into_string()
                .map_err(|_| std::io::Error::other("hostname is not UTF-8"))
        }) {
            Ok(name) => Some(name),
            Err(error) => {
                snapshot.errors.push(format!("hostname: {error}"));
                None
            }
        },
    };
    snapshot
}

/// Flatten the wire payload, preserving insertion order and omitting unavailable values.
pub fn to_json_dict(
    snapshot: &Snapshot,
    include_hostname: bool,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::json;
    let mut out = serde_json::Map::new();
    out.insert(
        "ts".into(),
        json!(snapshot.ts.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
    );
    out.insert("version".into(), json!(crate::consts::VERSION));
    if include_hostname && let Some(name) = &snapshot.hostname {
        out.insert("hostname".into(), json!(name));
    }
    if let Some(seconds) = snapshot.uptime_s {
        out.insert("uptime_s".into(), json!(seconds as u64));
        out.insert("uptime_minutes".into(), json!(snapshot.uptime_minutes));
    }
    for (field, value) in &snapshot.meminfo {
        out.insert(meminfo_topic_name(field), json!(value));
    }
    if !snapshot.errors.is_empty() {
        out.insert("errors".into(), json!(snapshot.errors));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEM: &str = "MemTotal: 1008432 kB\nMemAvailable: 457392 kB\nMemFree: 98964 kB\nSlab: 61204 kB\nHugePages_Total: 0\nBroken: notanumber kB\nmalformed\nNoColon 12\n";

    fn reader(path: &str) -> std::io::Result<String> {
        Ok(match path {
            crate::consts::PROC_UPTIME => "93120.42 0",
            crate::consts::PROC_MEMINFO => MEM,
            _ => panic!("unexpected path"),
        }
        .into())
    }

    #[test]
    fn snapshot_collects_and_serializes_flat_ordered_utc_payload() {
        let now = "2026-09-20T12:34:56.987Z".parse().unwrap();
        let options = CollectOptions {
            fields: &["MemAvailable", "Slab"],
            hostname: Some("testhost"),
            now: Some(now),
            ..Default::default()
        };
        let snap = collect_with_reader(&options, reader);
        assert!(snap.errors.is_empty());
        assert_eq!(snap.ts, now);
        assert_eq!(snap.uptime_minutes, Some(1552));
        assert_eq!(
            snap.meminfo,
            vec![("MemAvailable".into(), 457392), ("Slab".into(), 61204)]
        );
        let payload = to_json_dict(&snap, true);
        assert_eq!(payload["ts"], "2026-09-20T12:34:56Z");
        assert_eq!(payload["version"], crate::consts::VERSION);
        assert_eq!(payload["hostname"], "testhost");
        assert_eq!(payload["uptime_s"], 93120);
        assert_eq!(payload["uptime_minutes"], 1552);
        assert_eq!(payload["mem_available_kb"], 457392);
        assert!(!payload.contains_key("errors"));
        let wire = serde_json::to_string(&payload).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&wire).unwrap()["slab_kb"],
            61204
        );
        assert_eq!(
            payload.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "ts",
                "version",
                "hostname",
                "uptime_s",
                "uptime_minutes",
                "mem_available_kb",
                "slab_kb"
            ]
        );
    }

    #[test]
    fn snapshot_errors_are_isolated_and_missing_metrics_are_not_invented() {
        let options = CollectOptions {
            fields: &["MemAvailable"],
            ..Default::default()
        };
        for failed in [crate::consts::PROC_UPTIME, crate::consts::PROC_MEMINFO] {
            let snap = collect_with_reader(&options, |path| {
                if path == failed {
                    Err(std::io::Error::other("nope"))
                } else {
                    reader(path)
                }
            });
            assert_eq!(snap.errors, vec![format!("{failed}: nope")]);
            if failed == crate::consts::PROC_UPTIME {
                assert_eq!(snap.uptime_s, None);
                assert_eq!(snap.meminfo[0].1, 457392);
            } else {
                assert_eq!(snap.uptime_minutes, Some(1552));
                assert!(snap.meminfo.is_empty());
            }
            let json = to_json_dict(&snap, false);
            assert!(!json.contains_key("hostname"));
            assert!(json.contains_key("errors"));
            assert_eq!(
                json.contains_key("uptime_s"),
                failed != crate::consts::PROC_UPTIME
            );
        }
        let missing = collect_with_reader(
            &CollectOptions {
                fields: &["MemAvailable", "NoSuchField"],
                ..Default::default()
            },
            reader,
        );
        assert_eq!(
            missing.errors,
            vec!["/proc/meminfo: fields not present in this kernel: NoSuchField"]
        );
        assert_eq!(missing.meminfo[0].1, 457392);
        for bad in ["", "-1", "NaN", "inf"] {
            let snap = collect_with_reader(&options, |path| {
                if path == crate::consts::PROC_UPTIME {
                    Ok(bad.into())
                } else {
                    reader(path)
                }
            });
            assert_eq!(snap.uptime_s, None);
            assert_eq!(snap.uptime_minutes, None);
            assert_eq!(snap.errors.len(), 1);
        }
    }

    #[test]
    fn disabled_collectors_do_not_read_and_clock_is_real() {
        let before = chrono::Utc::now();
        let snap = collect_with_reader(
            &CollectOptions {
                want_uptime: false,
                want_meminfo: false,
                ..Default::default()
            },
            |_| panic!("disabled collector read"),
        );
        assert!(snap.errors.is_empty());
        assert!(snap.uptime_s.is_none() && snap.meminfo.is_empty());
        assert!(snap.ts >= before && snap.ts <= chrono::Utc::now());
        assert_eq!(to_json_dict(&snap, true)["ts"].as_str().unwrap().len(), 20);
        let snap = collect_with_reader(
            &CollectOptions {
                want_uptime: false,
                fields: &["Slab"],
                ..Default::default()
            },
            |path| {
                assert_eq!(path, crate::consts::PROC_MEMINFO);
                reader(path)
            },
        );
        assert_eq!(snap.meminfo[0].1, 61204);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_linux_proc_smoke_test() {
        let snap = collect(&CollectOptions::default());
        assert!(snap.errors.is_empty(), "{:?}", snap.errors);
        assert!(snap.uptime_s.unwrap() > 0.0);
        assert!(snap.meminfo.iter().any(|(k, v)| k == "MemTotal" && *v > 0));
        assert_eq!(
            snap.hostname.unwrap(),
            hostname::get().unwrap().into_string().unwrap()
        );
    }

    #[test]
    fn meminfo_preserves_selection_order_and_skips_bad_or_missing_fields() {
        assert_eq!(
            parse_meminfo(
                MEM,
                Some(&[
                    "MemAvailable",
                    "MemTotal",
                    "Broken",
                    "NoSuchField",
                    "MemAvailable"
                ])
            ),
            vec![
                ("MemAvailable".into(), 457392),
                ("MemTotal".into(), 1008432)
            ]
        );
        let all = parse_meminfo(MEM, None);
        assert_eq!(
            all.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            vec![
                "HugePages_Total",
                "MemAvailable",
                "MemFree",
                "MemTotal",
                "Slab"
            ]
        );
        assert_eq!(all[0].1, 0);
        assert_eq!(all[4].1, 61204);
        assert!(parse_meminfo(MEM, Some(&[])).is_empty());
        assert_eq!(
            parse_meminfo("A: 1\nA: 2\nA: broken", None),
            vec![("A".into(), 2)]
        );
        for (field, expected) in [
            ("MemAvailable", "mem_available_kb"),
            ("SUnreclaim", "sunreclaim_kb"),
            ("Slab", "slab_kb"),
            ("SomeNewKernelField", "some_new_kernel_field_kb"),
        ] {
            assert_eq!(meminfo_topic_name(field), expected);
        }
    }

    #[test]
    fn uptime_uses_first_field_and_truncates_minutes() {
        assert_eq!(parse_uptime_seconds("93120.42 1234.5\n").unwrap(), 93120.42);
        assert_eq!(parse_uptime_minutes("93120.42 1234.5").unwrap(), 1552);
        assert_eq!(parse_uptime_minutes("119.9 0.0").unwrap(), 1);
        assert_eq!(parse_uptime_minutes("0 0").unwrap(), 0);
        for text in [
            "",
            "  \n",
            "not-a-number 1",
            "-5 1",
            "NaN",
            "inf",
            "-inf",
            "1e100",
        ] {
            assert!(parse_uptime_seconds(text).is_err(), "{text}");
        }
    }
}
