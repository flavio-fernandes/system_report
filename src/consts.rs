//! Defaults and runtime constants, matching system_report/const.py.
pub const VERSION: &str = "1.0.0";
pub const APP_NAME: &str = "system_report";
pub const LOOP_TICK_MAX_SECS: f64 = 5.0;
pub const PENDING_RETRY_SECS: f64 = 1.0;
pub const DROP_LOG_INTERVAL_SECS: f64 = 300.0;
pub const MAX_CONSECUTIVE_CYCLE_FAILURES: u32 = 5;
pub const PROC_UPTIME: &str = "/proc/uptime";
pub const PROC_MEMINFO: &str = "/proc/meminfo";
pub const DEFAULTS_YAML: &str = include_str!("defaults.yaml");
pub const DEFAULT_MEMINFO_FIELDS: &[&str] = &[
    "MemTotal",
    "MemFree",
    "MemAvailable",
    "Buffers",
    "Cached",
    "Shmem",
    "Slab",
    "SReclaimable",
    "SUnreclaim",
    "KernelStack",
    "PageTables",
    "VmallocUsed",
    "SwapTotal",
    "SwapFree",
];
pub const MEMINFO_TOPIC_NAMES: &[(&str, &str)] = &[
    ("MemTotal", "mem_total_kb"),
    ("MemFree", "mem_free_kb"),
    ("MemAvailable", "mem_available_kb"),
    ("Buffers", "buffers_kb"),
    ("Cached", "cached_kb"),
    ("SwapCached", "swap_cached_kb"),
    ("SwapTotal", "swap_total_kb"),
    ("SwapFree", "swap_free_kb"),
    ("Shmem", "shmem_kb"),
    ("Slab", "slab_kb"),
    ("SReclaimable", "sreclaimable_kb"),
    ("SUnreclaim", "sunreclaim_kb"),
    ("KernelStack", "kernel_stack_kb"),
    ("PageTables", "page_tables_kb"),
    ("VmallocUsed", "vmalloc_used_kb"),
    ("Committed_AS", "committed_as_kb"),
    ("Dirty", "dirty_kb"),
    ("Writeback", "writeback_kb"),
    ("Mapped", "mapped_kb"),
    ("Active", "active_kb"),
    ("Inactive", "inactive_kb"),
];

/// Shared with the future collectors module; preserves the Python fallback.
pub fn meminfo_topic_name(field: &str) -> String {
    if let Some((_, name)) = MEMINFO_TOPIC_NAMES.iter().find(|(key, _)| *key == field) {
        return (*name).to_owned();
    }
    let mut name = String::new();
    let mut previous = None;
    for c in field.chars() {
        if previous.is_some_and(|p: char| p.is_ascii_lowercase() || p.is_ascii_digit())
            && c.is_ascii_uppercase()
        {
            name.push('_');
        }
        name.push(c);
        previous = Some(c);
    }
    format!("{}_kb", name.replace("__", "_").to_lowercase())
}
