import json
import re
from datetime import timedelta

import pytest

from system_report import collectors
from system_report.tests.unit.conftest import MEMINFO_SAMPLE, UPTIME_SAMPLE


# --- /proc/uptime -----------------------------------------------------------


def test_uptime_seconds_and_minutes_come_from_the_first_field():
    assert collectors.parse_uptime_seconds(UPTIME_SAMPLE) == pytest.approx(93120.42)
    assert collectors.parse_uptime_minutes(UPTIME_SAMPLE) == 1552


def test_uptime_minutes_truncates_rather_than_rounds():
    assert collectors.parse_uptime_minutes("119.9 0.0") == 1


@pytest.mark.parametrize("text", ["", "   \n", "not-a-number 1.0", "-5.0 1.0"])
def test_bad_uptime_raises(text):
    with pytest.raises(ValueError):
        collectors.parse_uptime_seconds(text)


# --- /proc/meminfo ----------------------------------------------------------


def test_meminfo_returns_requested_fields_in_requested_order():
    values = collectors.parse_meminfo(MEMINFO_SAMPLE, ["MemAvailable", "MemTotal"])
    assert list(values.items()) == [("MemAvailable", 457392), ("MemTotal", 1008432)]


def test_meminfo_without_a_field_list_returns_everything_sorted():
    values = collectors.parse_meminfo(MEMINFO_SAMPLE)
    assert values["Slab"] == 61204
    # unitless lines (HugePages_*) are values too
    assert values["HugePages_Total"] == 0
    assert list(values) == sorted(values)


def test_meminfo_missing_field_is_omitted_not_invented():
    values = collectors.parse_meminfo(MEMINFO_SAMPLE, ["MemFree", "NoSuchField"])
    assert list(values) == ["MemFree"]


def test_one_malformed_line_does_not_cost_the_other_fields():
    text = "MemTotal:        1008432 kB\nBroken: notanumber kB\nMemFree: 98964 kB\n"
    values = collectors.parse_meminfo(text, ["MemTotal", "Broken", "MemFree"])
    assert list(values.items()) == [("MemTotal", 1008432), ("MemFree", 98964)]


@pytest.mark.parametrize("field,expected", [
    ("MemAvailable", "mem_available_kb"),   # bedclock compatible
    ("SUnreclaim", "sunreclaim_kb"),
    ("Slab", "slab_kb"),
    ("SomeNewKernelField", "some_new_kernel_field_kb"),
])
def test_topic_leaf_names(field, expected):
    assert collectors.meminfo_topic_name(field) == expected


# --- snapshots --------------------------------------------------------------


def test_collect_gathers_uptime_and_meminfo(proc_reader):
    snap = collectors.collect(fields=["MemAvailable", "Slab"], reader=proc_reader(),
                              hostname="testhost")
    assert snap.errors == []
    assert snap.uptime_minutes == 1552
    assert list(snap.meminfo.items()) == [("MemAvailable", 457392), ("Slab", 61204)]
    assert snap.hostname == "testhost"


def test_unreadable_uptime_does_not_stop_meminfo(proc_reader):
    reader = proc_reader({"/proc/uptime": IOError("nope")})
    snap = collectors.collect(fields=["MemAvailable"], reader=reader)
    assert snap.uptime_minutes is None
    assert snap.meminfo["MemAvailable"] == 457392
    assert len(snap.errors) == 1 and "/proc/uptime" in snap.errors[0]


def test_unreadable_meminfo_does_not_stop_uptime(proc_reader):
    reader = proc_reader({"/proc/meminfo": IOError("nope")})
    snap = collectors.collect(fields=["MemAvailable"], reader=reader)
    assert snap.uptime_minutes == 1552
    assert snap.meminfo == {}
    assert len(snap.errors) == 1 and "/proc/meminfo" in snap.errors[0]


def test_missing_field_is_reported_as_an_error(proc_reader):
    snap = collectors.collect(fields=["MemAvailable", "NoSuchField"], reader=proc_reader())
    assert "NoSuchField" in snap.errors[0]
    assert snap.meminfo["MemAvailable"] == 457392


def test_disabled_collectors_are_not_read(proc_reader):
    reader = proc_reader({"/proc/uptime": AssertionError("should not be read")})
    snap = collectors.collect(fields=["MemAvailable"], reader=reader, want_uptime=False)
    assert snap.uptime_s is None and snap.errors == []


def test_json_dict_is_flat_and_serializable(proc_reader):
    snap = collectors.collect(fields=["MemAvailable", "Slab"], reader=proc_reader(),
                              hostname="testhost")
    payload = collectors.to_json_dict(snap)
    assert payload["hostname"] == "testhost"
    assert payload["mem_available_kb"] == 457392
    assert payload["uptime_s"] == 93120
    assert payload["ts"].endswith("Z")
    assert "errors" not in payload
    assert json.loads(json.dumps(payload))["slab_kb"] == 61204


def test_timestamp_is_timezone_aware_utc_and_keeps_its_wire_format(proc_reader):
    # datetime.utcnow() returns a naive value and is deprecated from 3.12 on,
    # heading for removal; timezone.utc works on 3.6 through 3.14 alike. The
    # published string must not change shape either way -- subscribers parse it.
    snap = collectors.collect(fields=["MemAvailable"], reader=proc_reader())
    assert snap.ts.tzinfo is not None
    assert snap.ts.utcoffset() == timedelta(0)
    assert re.match(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$",
                    collectors.to_json_dict(snap)["ts"])


def test_json_dict_carries_errors_and_can_omit_the_hostname(proc_reader):
    reader = proc_reader({"/proc/uptime": IOError("nope")})
    snap = collectors.collect(fields=["MemAvailable"], reader=reader)
    payload = collectors.to_json_dict(snap, include_hostname=False)
    assert "hostname" not in payload
    assert "uptime_s" not in payload
    assert payload["errors"]
