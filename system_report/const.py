#!/usr/bin/env python3
"""Every default value used by system_report lives in this file.

If a knob is missing from your config.yaml, the value below is what you get.
``data/config.yaml.example`` mirrors this file with prose, so the two should be
kept in sync when a knob is added.

Nothing here is host specific: no real broker addresses, hostnames or secrets.
"""

VERSION = "1.0.0"
APP_NAME = "system_report"

# --- loop / runtime tunables (not exposed in config.yaml on purpose) ---------

# Longest the main loop sleeps between ticks. It bounds how quickly the process
# reacts to SIGTERM and how often the systemd watchdog is petted.
LOOP_TICK_MAX_SECS = 5.0

# When a report is due but the broker is not connected yet, retry this often.
PENDING_RETRY_SECS = 1.0

# Publishing while disconnected is normal during an outage; log it at most this
# often, with a count of how many reports were dropped meanwhile.
DROP_LOG_INTERVAL_SECS = 300.0

# Give up (exit non-zero, let systemd restart us) after this many consecutive
# failed loop iterations. A single failure is logged and retried.
MAX_CONSECUTIVE_CYCLE_FAILURES = 5

PROC_UPTIME = "/proc/uptime"
PROC_MEMINFO = "/proc/meminfo"

# --- meminfo field -> topic leaf name ---------------------------------------
# /proc/meminfo uses CamelCase; MQTT topics here use snake_case with the unit
# spelled out. The two names below marked "bedclock" are deliberately identical
# to what the bedclock project publishes, so existing subscribers keep working.
MEMINFO_TOPIC_NAMES = {
    "MemTotal": "mem_total_kb",
    "MemFree": "mem_free_kb",
    "MemAvailable": "mem_available_kb",      # bedclock: oper_state/mem_available_kb
    "Buffers": "buffers_kb",
    "Cached": "cached_kb",
    "SwapCached": "swap_cached_kb",
    "SwapTotal": "swap_total_kb",
    "SwapFree": "swap_free_kb",
    "Shmem": "shmem_kb",
    "Slab": "slab_kb",
    "SReclaimable": "sreclaimable_kb",
    "SUnreclaim": "sunreclaim_kb",
    "KernelStack": "kernel_stack_kb",
    "PageTables": "page_tables_kb",
    "VmallocUsed": "vmalloc_used_kb",
    "Committed_AS": "committed_as_kb",
    "Dirty": "dirty_kb",
    "Writeback": "writeback_kb",
    "Mapped": "mapped_kb",
    "Active": "active_kb",
    "Inactive": "inactive_kb",
}

# The default field list is the leak-attribution set: MemAvailable answers "how
# much headroom is left", and the rest answer "which bucket is eating it".
DEFAULT_MEMINFO_FIELDS = [
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
]

# --- config defaults --------------------------------------------------------
# Anything the config file does not set falls back to these values.
DEFAULTS = {
    "mqtt": {
        "host": "localhost",
        "port": 1883,
        # None -> "system_report_{hostname}"
        "client_id": None,
        "keepalive_secs": 60,
        "username": None,
        # Secret precedence: password_file, then password_env, then password.
        "password": None,
        "password_file": None,
        "password_env": "SYSTEM_REPORT_MQTT_PASSWORD",
        "qos": 0,
        "retain": False,
        "clean_session": True,
        "reconnect_min_delay_secs": 1,
        "reconnect_max_delay_secs": 120,
        # Rebuild the client from scratch after this long with no connection.
        # 0 disables the safety net and leaves reconnects to paho alone.
        "recreate_client_after_secs": 900,
        "publish_timeout_secs": 10.0,
        # Caps paho's internal queue, which is unbounded by default. Reports
        # are dropped rather than buffered while the broker is unreachable.
        "max_queued_messages": 100,
        "tls": {
            "enabled": False,
            "ca_certs": None,
            "certfile": None,
            "keyfile": None,
            # True skips server certificate/hostname verification. Testing only.
            "insecure": False,
        },
    },
    "topics": {
        # "{hostname}" is expanded once at startup. Override the prefix if you
        # would rather not publish this machine's hostname.
        "prefix": "/{hostname}",
        "status": "{prefix}/oper_state/status",
        "json": "{prefix}/oper_state/json",
        "payload_online": "online",
        "payload_offline": "offline",
        "status_retain": True,
    },
    "report": {
        "interval_secs": 600,
        "report_on_start": True,
        "publish_plain": True,
        "publish_json": True,
    },
    "metrics": {
        "uptime_minutes": {
            "enabled": True,
            "topic": "{prefix}/oper_uptime_minutes",   # bedclock compatible
        },
        "meminfo": {
            "enabled": True,
            # "{name}" expands to the topic leaf name of each field, e.g.
            # mem_available_kb (see MEMINFO_TOPIC_NAMES above).
            "topic": "{prefix}/oper_state/{name}",
            "fields": list(DEFAULT_MEMINFO_FIELDS),
        },
    },
    "knobs": {
        "log_to_console": False,
        "log_level_debug": False,
    },
}
