#!/usr/bin/env python3
"""Loading, validating and templating the YAML configuration.

Design rules:

* every knob has a default in :mod:`system_report.const`, so config.yaml only
  needs to carry what differs;
* a bad value fails at startup with a message naming the key, rather than at
  3 a.m. inside the publish path;
* an unknown key is a warning, not a silent no-op, because a typo in a knob is
  otherwise invisible;
* secrets are read from a file or the environment by preference, and are never
  logged or returned by :meth:`Config.redacted`.
"""

import copy
import os
import socket

import yaml

from system_report import collectors
from system_report import const
from system_report import log

REDACTED = "<redacted>"

# Default location: data/config.yaml next to this checkout.
CFG_FILENAME = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "data", "config.yaml"
)

_SECRET_KEYS = ("password",)


class ConfigError(Exception):
    """Raised for anything that makes the configuration unusable."""


# --- merging ----------------------------------------------------------------


def _deep_merge(base, override, path, unknown):
    """Overlay ``override`` on ``base``. Lists replace, dicts merge."""
    merged = copy.deepcopy(base)
    for key, value in (override or {}).items():
        where = "{}.{}".format(path, key) if path else str(key)
        if key not in merged:
            unknown.append(where)
            merged[key] = value
        elif isinstance(merged[key], dict) and isinstance(value, dict):
            merged[key] = _deep_merge(merged[key], value, where, unknown)
        else:
            merged[key] = value
    return merged


# --- validation -------------------------------------------------------------


def _fail(where, message):
    raise ConfigError("config key '{}': {}".format(where, message))


def _check_bool(cfg, where):
    value = _dig(cfg, where)
    if not isinstance(value, bool):
        _fail(where, "expected true/false, got {!r}".format(value))


def _check_int(cfg, where, minimum=None, maximum=None):
    value = _dig(cfg, where)
    if isinstance(value, bool) or not isinstance(value, int):
        _fail(where, "expected an integer, got {!r}".format(value))
    if minimum is not None and value < minimum:
        _fail(where, "must be >= {}, got {}".format(minimum, value))
    if maximum is not None and value > maximum:
        _fail(where, "must be <= {}, got {}".format(maximum, value))


def _check_number(cfg, where, minimum=None):
    value = _dig(cfg, where)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        _fail(where, "expected a number, got {!r}".format(value))
    if minimum is not None and value < minimum:
        _fail(where, "must be >= {}, got {}".format(minimum, value))


def _check_str(cfg, where, allow_none=False, allow_empty=False):
    value = _dig(cfg, where)
    if value is None:
        if allow_none:
            return
        _fail(where, "must be set")
    if not isinstance(value, str):
        _fail(where, "expected a string, got {!r}".format(value))
    if not allow_empty and not value.strip():
        _fail(where, "must not be empty")


def _check_topic(cfg, where, must_contain=None):
    _check_str(cfg, where)
    value = _dig(cfg, where)
    for bad in ("+", "#", "\x00"):
        if bad in value:
            _fail(where, "a publish topic cannot contain {!r}".format(bad))
    if must_contain and must_contain not in value:
        _fail(where, "must contain {!r} so each field gets its own topic".format(must_contain))


def _dig(cfg, dotted):
    node = cfg
    for part in dotted.split("."):
        node = node[part]
    return node


def _validate(cfg):
    _check_str(cfg, "mqtt.host")
    _check_int(cfg, "mqtt.port", 1, 65535)
    _check_str(cfg, "mqtt.client_id", allow_none=True)
    _check_int(cfg, "mqtt.keepalive_secs", 5, 65535)
    _check_str(cfg, "mqtt.username", allow_none=True)
    _check_str(cfg, "mqtt.password", allow_none=True)
    _check_str(cfg, "mqtt.password_file", allow_none=True)
    _check_str(cfg, "mqtt.password_env", allow_none=True)
    _check_int(cfg, "mqtt.qos", 0, 2)
    _check_bool(cfg, "mqtt.retain")
    _check_bool(cfg, "mqtt.clean_session")
    _check_int(cfg, "mqtt.reconnect_min_delay_secs", 1)
    _check_int(cfg, "mqtt.reconnect_max_delay_secs", 1)
    if cfg["mqtt"]["reconnect_max_delay_secs"] < cfg["mqtt"]["reconnect_min_delay_secs"]:
        _fail("mqtt.reconnect_max_delay_secs", "must be >= mqtt.reconnect_min_delay_secs")
    _check_int(cfg, "mqtt.recreate_client_after_secs", 0)
    _check_number(cfg, "mqtt.publish_timeout_secs", 0.1)
    _check_int(cfg, "mqtt.max_queued_messages", 0)
    _check_bool(cfg, "mqtt.tls.enabled")
    for key in ("ca_certs", "certfile", "keyfile"):
        _check_str(cfg, "mqtt.tls." + key, allow_none=True)
    _check_bool(cfg, "mqtt.tls.insecure")

    _check_str(cfg, "topics.prefix", allow_empty=True)
    _check_topic(cfg, "topics.status")
    _check_topic(cfg, "topics.json")
    _check_str(cfg, "topics.payload_online")
    _check_str(cfg, "topics.payload_offline")
    _check_bool(cfg, "topics.status_retain")

    _check_int(cfg, "report.interval_secs", 1)
    _check_bool(cfg, "report.report_on_start")
    _check_bool(cfg, "report.publish_plain")
    _check_bool(cfg, "report.publish_json")

    _check_bool(cfg, "metrics.uptime_minutes.enabled")
    _check_topic(cfg, "metrics.uptime_minutes.topic")
    _check_bool(cfg, "metrics.meminfo.enabled")
    _check_topic(cfg, "metrics.meminfo.topic", must_contain="{name}")
    fields = cfg["metrics"]["meminfo"]["fields"]
    if not isinstance(fields, list) or not fields:
        _fail("metrics.meminfo.fields", "expected a non-empty list of /proc/meminfo names")
    for field in fields:
        if not isinstance(field, str) or not field.strip():
            _fail("metrics.meminfo.fields", "bad entry {!r}".format(field))

    _check_bool(cfg, "knobs.log_to_console")
    _check_bool(cfg, "knobs.log_level_debug")

    metrics = cfg["metrics"]
    if not metrics["uptime_minutes"]["enabled"] and not metrics["meminfo"]["enabled"]:
        _fail("metrics", "every metric is disabled; there would be nothing to report")
    if not cfg["report"]["publish_plain"] and not cfg["report"]["publish_json"]:
        _fail("report", "publish_plain and publish_json are both false; nothing would be sent")


# --- secrets ----------------------------------------------------------------


def _mode_warning(path):
    """Return a warning if ``path`` is readable by group or others."""
    try:
        mode = os.stat(path).st_mode
    except OSError:
        return None
    if mode & 0o077:
        return ("{} is readable beyond its owner (mode {:04o}); "
                "chmod 600 it".format(path, mode & 0o7777))
    return None


def _resolve_password(mqtt, env, config_path, warnings):
    password_file = mqtt.get("password_file")
    if password_file:
        path = os.path.expanduser(password_file)
        try:
            with open(path, "r") as handle:
                secret = handle.read().strip()
        except (OSError, IOError) as e:
            raise ConfigError("cannot read mqtt.password_file {}: {}".format(path, e))
        if not secret:
            raise ConfigError("mqtt.password_file {} is empty".format(path))
        warning = _mode_warning(path)
        if warning:
            warnings.append(warning)
        return secret

    env_name = mqtt.get("password_env")
    if env_name:
        secret = env.get(env_name)
        if secret:
            return secret

    inline = mqtt.get("password")
    if inline:
        warnings.append(
            "mqtt.password is set inline in the config file; mqtt.password_file or the "
            "{} environment variable keeps the secret out of it".format(
                mqtt.get("password_env") or "password_env"
            )
        )
        if config_path:
            warning = _mode_warning(config_path)
            if warning:
                warnings.append(warning)
        return inline

    return None


# --- the config object ------------------------------------------------------


class Config(object):
    def __init__(self, data, path=None, hostname=None, env=None, unknown_keys=None):
        self.path = path
        self.hostname = hostname or socket.gethostname()
        self.warnings = []
        for key in unknown_keys or []:
            self.warnings.append("unknown config key '{}' (ignored, check for a typo)".format(key))

        self.mqtt = data["mqtt"]
        self.topics = data["topics"]
        self.report = data["report"]
        self.metrics = data["metrics"]
        self.knobs = data["knobs"]
        self._data = data

        self.prefix = self._format(self.topics["prefix"], "topics.prefix").rstrip("/")
        self.status_topic = self._topic(self.topics["status"], "topics.status")
        self.json_topic = self._topic(self.topics["json"], "topics.json")
        self.uptime_topic = self._topic(
            self.metrics["uptime_minutes"]["topic"], "metrics.uptime_minutes.topic"
        )
        self._meminfo_topic_template = self.metrics["meminfo"]["topic"]

        self.password = _resolve_password(self.mqtt, env or os.environ, path, self.warnings)
        if self.mqtt.get("username") and not self.password:
            self.warnings.append(
                "mqtt.username is set but no password was found; connecting without one"
            )
        if self.mqtt["tls"]["enabled"] and self.mqtt["tls"]["insecure"]:
            self.warnings.append(
                "mqtt.tls.insecure is true: the broker certificate is NOT verified"
            )
        if not self.mqtt["tls"]["enabled"] and self.password:
            self.warnings.append(
                "TLS is disabled, so the MQTT password crosses the network in the clear"
            )

    # -- templating

    def _format(self, template, where, **extra):
        try:
            return template.format(hostname=self.hostname, prefix=getattr(self, "prefix", ""),
                                   **extra)
        except (KeyError, IndexError) as e:
            raise ConfigError(
                "config key '{}': unknown placeholder {} in {!r}".format(where, e, template)
            )

    def _topic(self, template, where, **extra):
        topic = self._format(template, where, **extra)
        if not topic:
            raise ConfigError("config key '{}': resolved to an empty topic".format(where))
        return topic

    def meminfo_topic(self, field):
        name = collectors.meminfo_topic_name(field)
        return self._topic(self._meminfo_topic_template, "metrics.meminfo.topic", name=name)

    # -- convenience

    @property
    def client_id(self):
        return self.mqtt.get("client_id") or "{}_{}".format(const.APP_NAME, self.hostname)

    @property
    def meminfo_fields(self):
        return list(self.metrics["meminfo"]["fields"])

    @property
    def uptime_enabled(self):
        return bool(self.metrics["uptime_minutes"]["enabled"])

    @property
    def meminfo_enabled(self):
        return bool(self.metrics["meminfo"]["enabled"])

    def redacted(self):
        """A deep copy safe to log: secrets replaced by a marker."""
        out = copy.deepcopy(self._data)
        for key in _SECRET_KEYS:
            if out["mqtt"].get(key):
                out["mqtt"][key] = REDACTED
        return out

    def log_warnings(self, logger=None):
        logger = logger or log.getLogger()
        for warning in self.warnings:
            logger.warning("config: %s", warning)


# --- entry point ------------------------------------------------------------


def load(path=None, env=None, hostname=None):
    """Read a config file (or pure defaults when it does not exist)."""
    path = path or CFG_FILENAME
    raw = {}
    if os.path.exists(path):
        try:
            with open(path, "r") as handle:
                raw = yaml.safe_load(handle) or {}
        except yaml.YAMLError as e:
            raise ConfigError("cannot parse {}: {}".format(path, e))
        except (OSError, IOError) as e:
            raise ConfigError("cannot read {}: {}".format(path, e))
        if not isinstance(raw, dict):
            raise ConfigError("{}: expected a mapping at the top level".format(path))
    elif path != CFG_FILENAME:
        raise ConfigError("config file not found: {}".format(path))

    unknown = []
    merged = _deep_merge(const.DEFAULTS, raw, "", unknown)
    _validate(merged)
    exists = os.path.exists(path)
    cfg = Config(merged, path=path if exists else None,
                 hostname=hostname, env=env, unknown_keys=unknown)
    if not exists:
        cfg.warnings.insert(0, "no config file at {}; using built-in defaults "
                               "(broker {}:{})".format(path, cfg.mqtt["host"], cfg.mqtt["port"]))
    return cfg
