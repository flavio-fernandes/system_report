#!/usr/bin/env python3
"""Reading Linux operational state out of /proc.

Everything here is a pure function over text plus a thin reader, so the parsers
are unit testable without a Linux box and without mocking the filesystem: pass
your own ``reader`` and you control the input.

A metric that cannot be read never takes the others down with it: failures are
collected into ``Snapshot.errors`` and the rest of the report still goes out.
"""

import collections
import re
import socket
from datetime import datetime

from system_report import const

Snapshot = collections.namedtuple(
    "Snapshot", "ts hostname uptime_s uptime_minutes meminfo errors"
)

_CAMEL_BOUNDARY = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")


def read_text(path):
    """Default reader: the whole file as text."""
    with open(path, "r") as handle:
        return handle.read()


# --- /proc/uptime -----------------------------------------------------------


def parse_uptime_seconds(text):
    fields = text.split()
    if not fields:
        raise ValueError("missing uptime seconds")
    uptime_seconds = float(fields[0])
    if uptime_seconds < 0:
        raise ValueError("negative uptime seconds")
    return uptime_seconds


def parse_uptime_minutes(text):
    return int(parse_uptime_seconds(text) / 60)


# --- /proc/meminfo ----------------------------------------------------------


def parse_meminfo(text, fields=None):
    """Return an OrderedDict of meminfo values, in the order of ``fields``.

    Values are the raw kB numbers the kernel reports. Lines that cannot be
    parsed are skipped rather than raising, so one odd line in a kernel we have
    never seen cannot cost us every other field; a requested field that ends up
    missing is reported by the caller.
    """
    parsed = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) < 2 or not parts[0].endswith(":"):
            continue
        try:
            parsed[parts[0][:-1]] = int(parts[1])
        except ValueError:
            continue

    if fields is None:
        fields = sorted(parsed)

    values = collections.OrderedDict()
    for field in fields:
        if field in parsed:
            values[field] = parsed[field]
    return values


def meminfo_topic_name(field):
    """Map a /proc/meminfo field name to the leaf name used in topics/JSON."""
    known = const.MEMINFO_TOPIC_NAMES.get(field)
    if known:
        return known
    return _CAMEL_BOUNDARY.sub("_", field).replace("__", "_").lower() + "_kb"


# --- snapshot ---------------------------------------------------------------


def collect(fields=None, reader=read_text, want_uptime=True, want_meminfo=True,
            hostname=None, now=None):
    """Take one snapshot. Never raises: problems land in ``Snapshot.errors``."""
    if fields is None:
        fields = const.DEFAULT_MEMINFO_FIELDS
    errors = []
    uptime_s = None
    uptime_minutes = None
    meminfo = collections.OrderedDict()

    if want_uptime:
        try:
            uptime_s = parse_uptime_seconds(reader(const.PROC_UPTIME))
            uptime_minutes = int(uptime_s / 60)
        except Exception as e:
            errors.append("{}: {}".format(const.PROC_UPTIME, e))

    if want_meminfo:
        try:
            meminfo = parse_meminfo(reader(const.PROC_MEMINFO), fields)
            missing = [f for f in fields if f not in meminfo]
            if missing:
                errors.append(
                    "{}: fields not present in this kernel: {}".format(
                        const.PROC_MEMINFO, ", ".join(missing)
                    )
                )
        except Exception as e:
            errors.append("{}: {}".format(const.PROC_MEMINFO, e))

    return Snapshot(
        ts=now or datetime.utcnow(),
        hostname=hostname or socket.gethostname(),
        uptime_s=uptime_s,
        uptime_minutes=uptime_minutes,
        meminfo=meminfo,
        errors=errors,
    )


def to_json_dict(snapshot, include_hostname=True):
    """Flatten a snapshot for the JSON topic (and for offline analysis)."""
    out = collections.OrderedDict()
    out["ts"] = snapshot.ts.strftime("%Y-%m-%dT%H:%M:%SZ")
    out["version"] = const.VERSION
    if include_hostname:
        out["hostname"] = snapshot.hostname
    if snapshot.uptime_s is not None:
        out["uptime_s"] = int(snapshot.uptime_s)
        out["uptime_minutes"] = snapshot.uptime_minutes
    for field, value in snapshot.meminfo.items():
        out[meminfo_topic_name(field)] = value
    if snapshot.errors:
        out["errors"] = list(snapshot.errors)
    return out
