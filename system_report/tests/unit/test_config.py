import os

import pytest

from system_report import config


# --- merging and defaults ---------------------------------------------------


def test_defaults_apply_and_overrides_win(make_cfg):
    cfg = make_cfg({"mqtt": {"host": "broker.example.lan"}, "report": {"interval_secs": 30}})
    assert cfg.mqtt["host"] == "broker.example.lan"
    assert cfg.mqtt["port"] == 1883              # untouched default
    assert cfg.report["interval_secs"] == 30
    assert cfg.report["publish_json"] is True


def test_a_list_is_replaced_not_merged(make_cfg):
    cfg = make_cfg({"metrics": {"meminfo": {"fields": ["MemAvailable"]}}})
    assert cfg.meminfo_fields == ["MemAvailable"]


def test_unknown_key_is_kept_but_warned_about(make_cfg):
    cfg = make_cfg({"mqtt": {"hostname": "typo.example.lan"}})
    assert any("mqtt.hostname" in w for w in cfg.warnings)


def test_missing_config_file_is_an_error_when_explicitly_named(tmp_path):
    with pytest.raises(config.ConfigError):
        config.load(path=os.path.join(str(tmp_path), "does-not-exist.yaml"), env={})


def test_non_mapping_config_is_rejected(tmp_path):
    path = os.path.join(str(tmp_path), "config.yaml")
    with open(path, "w") as handle:
        handle.write("just a string\n")
    with pytest.raises(config.ConfigError):
        config.load(path=path, env={})


# --- validation -------------------------------------------------------------


@pytest.mark.parametrize("overrides", [
    {"mqtt": {"port": 0}},
    {"mqtt": {"port": "1883"}},
    {"mqtt": {"host": ""}},
    {"mqtt": {"qos": 3}},
    {"mqtt": {"retain": "yes"}},
    {"mqtt": {"keepalive_secs": 1}},
    {"mqtt": {"reconnect_min_delay_secs": 60, "reconnect_max_delay_secs": 30}},
    {"mqtt": {"publish_timeout_secs": 0}},
    {"report": {"interval_secs": 0}},
    {"report": {"publish_plain": False, "publish_json": False}},
    {"metrics": {"meminfo": {"fields": []}}},
    {"metrics": {"meminfo": {"fields": "MemAvailable"}}},
    # every field would collide on one topic without {name}
    {"metrics": {"meminfo": {"topic": "{prefix}/mem"}}},
    # publish topics cannot hold wildcards
    {"topics": {"status": "{prefix}/#"}},
    {"topics": {"json": "{prefix}/+/json"}},
    # nothing left to report
    {"metrics": {"meminfo": {"enabled": False}, "uptime_minutes": {"enabled": False}}},
    # unknown placeholder
    {"topics": {"prefix": "/{nope}"}},
])
def test_bad_config_fails_fast_with_the_key_named(make_cfg, overrides):
    with pytest.raises(config.ConfigError):
        make_cfg(overrides)


# --- topics -----------------------------------------------------------------


def test_topics_expand_hostname_and_prefix(make_cfg):
    cfg = make_cfg({}, hostname="myhost")
    assert cfg.prefix == "/myhost"
    assert cfg.status_topic == "/myhost/oper_state/status"
    assert cfg.json_topic == "/myhost/oper_state/json"
    assert cfg.uptime_topic == "/myhost/oper_uptime_minutes"
    assert cfg.meminfo_topic("MemAvailable") == "/myhost/oper_state/mem_available_kb"
    assert cfg.meminfo_topic("SUnreclaim") == "/myhost/oper_state/sunreclaim_kb"


def test_prefix_can_hide_the_hostname(make_cfg):
    cfg = make_cfg({"topics": {"prefix": "/sensors/box1/"}}, hostname="myhost")
    assert cfg.prefix == "/sensors/box1"          # trailing slash normalized away
    assert cfg.status_topic == "/sensors/box1/oper_state/status"


def test_client_id_defaults_to_app_and_hostname(make_cfg):
    assert make_cfg({}, hostname="myhost").client_id == "system_report_myhost"
    assert make_cfg({"mqtt": {"client_id": "box1"}}).client_id == "box1"


# --- secrets ----------------------------------------------------------------


def _write(path, content, mode=0o600):
    with open(path, "w") as handle:
        handle.write(content)
    os.chmod(path, mode)
    return path


def test_password_file_wins_and_is_stripped(tmp_path, make_cfg):
    secret = _write(os.path.join(str(tmp_path), "pw"), "s3cret\n")
    cfg = make_cfg({"mqtt": {"password_file": secret, "password": "inline"}},
                   env={"SYSTEM_REPORT_MQTT_PASSWORD": "from-env"})
    assert cfg.password == "s3cret"
    assert cfg.warnings == [] or all("readable beyond" not in w for w in cfg.warnings)


def test_password_file_readable_by_others_is_flagged(tmp_path, make_cfg):
    secret = _write(os.path.join(str(tmp_path), "pw"), "s3cret\n", mode=0o644)
    cfg = make_cfg({"mqtt": {"password_file": secret}})
    assert any("readable beyond its owner" in w for w in cfg.warnings)


def test_empty_or_unreadable_password_file_is_an_error(tmp_path, make_cfg):
    empty = _write(os.path.join(str(tmp_path), "empty"), "\n")
    with pytest.raises(config.ConfigError):
        make_cfg({"mqtt": {"password_file": empty}})
    with pytest.raises(config.ConfigError):
        make_cfg({"mqtt": {"password_file": os.path.join(str(tmp_path), "nope")}})


def test_environment_beats_inline(make_cfg):
    cfg = make_cfg({"mqtt": {"password": "inline"}},
                   env={"SYSTEM_REPORT_MQTT_PASSWORD": "from-env"})
    assert cfg.password == "from-env"


def test_inline_password_is_used_but_warned_about(make_cfg):
    cfg = make_cfg({"mqtt": {"username": "bob", "password": "inline"}})
    assert cfg.password == "inline"
    assert any("inline" in w for w in cfg.warnings)
    assert any("clear" in w for w in cfg.warnings)      # no TLS


def test_username_without_password_is_warned_about(make_cfg):
    cfg = make_cfg({"mqtt": {"username": "bob"}})
    assert cfg.password is None
    assert any("no password" in w for w in cfg.warnings)


def test_insecure_tls_is_warned_about(make_cfg):
    cfg = make_cfg({"mqtt": {"tls": {"enabled": True, "insecure": True}}})
    assert any("NOT verified" in w for w in cfg.warnings)


def test_redacted_config_never_carries_the_secret(make_cfg):
    cfg = make_cfg({"mqtt": {"password": "s3cret"}})
    redacted = cfg.redacted()
    assert redacted["mqtt"]["password"] == config.REDACTED
    assert "s3cret" not in str(redacted)
    assert cfg.password == "s3cret"      # the live value still works
