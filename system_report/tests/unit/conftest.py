"""Shared fixtures: a config factory and the fakes the tests publish through."""

import os

import pytest
import yaml

from system_report import config

MEMINFO_SAMPLE = """\
MemTotal:        1008432 kB
MemFree:           98964 kB
MemAvailable:     457392 kB
Buffers:           25868 kB
Cached:           443364 kB
SwapTotal:       2097148 kB
SwapFree:        2097112 kB
Shmem:              1296 kB
Slab:              61204 kB
SReclaimable:      43068 kB
SUnreclaim:        18136 kB
KernelStack:        3168 kB
PageTables:         4900 kB
VmallocUsed:           0 kB
HugePages_Total:       0
"""

UPTIME_SAMPLE = "93120.42 180000.11\n"


@pytest.fixture
def make_cfg(tmp_path):
    """Write a config.yaml under tmp_path and load it, with overrides applied."""

    def _make(overrides=None, env=None, hostname="testhost", filename="config.yaml"):
        path = os.path.join(str(tmp_path), filename)
        with open(path, "w") as handle:
            yaml.safe_dump(overrides or {}, handle, default_flow_style=False)
        return config.load(path=path, env=env if env is not None else {}, hostname=hostname)

    return _make


@pytest.fixture
def proc_reader():
    """A /proc reader backed by the samples above; raise by passing an exception."""

    def _reader(files=None):
        contents = {"/proc/uptime": UPTIME_SAMPLE, "/proc/meminfo": MEMINFO_SAMPLE}
        contents.update(files or {})

        def _read(path):
            value = contents[path]
            if isinstance(value, Exception):
                raise value
            return value

        return _read

    return _reader


class FakeClock(object):
    """Monotonic clock under test control."""

    def __init__(self, now=1000.0):
        self.now = now

    def __call__(self):
        return self.now

    def advance(self, seconds):
        self.now += seconds
        return self.now


class FakePublishInfo(object):
    def __init__(self, rc=0, published=True):
        self.rc = rc
        self._published = published
        self.waited = None

    def wait_for_publish(self, timeout=None):
        self.waited = timeout

    def is_published(self):
        return self._published


class FakeMqttClient(object):
    """Just enough of the paho API for Publisher to drive."""

    def __init__(self, client_id="", clean_session=True):
        self.client_id = client_id
        self.clean_session = clean_session
        self.published = []
        self.will = None
        self.credentials = None
        self.tls = None
        self.tls_insecure = False
        self.reconnect_delay = None
        self.max_queued = None
        self.connect_args = None
        self.loop_started = False
        self.loop_stopped = False
        self.disconnected = False
        self.on_connect = None
        self.on_disconnect = None
        self.publish_rc = 0
        self.publish_published = True

    def username_pw_set(self, username, password=None):
        self.credentials = (username, password)

    def tls_set(self, ca_certs=None, certfile=None, keyfile=None):
        self.tls = (ca_certs, certfile, keyfile)

    def tls_insecure_set(self, value):
        self.tls_insecure = value

    def reconnect_delay_set(self, min_delay=1, max_delay=120):
        self.reconnect_delay = (min_delay, max_delay)

    def max_queued_messages_set(self, value):
        self.max_queued = value

    def will_set(self, topic, payload=None, qos=0, retain=False):
        self.will = (topic, payload, qos, retain)

    def connect_async(self, host, port=1883, keepalive=60):
        self.connect_args = (host, port, keepalive)

    def loop_start(self):
        self.loop_started = True

    def loop_stop(self):
        self.loop_stopped = True

    def disconnect(self):
        self.disconnected = True

    def publish(self, topic, payload=None, qos=0, retain=False):
        self.published.append((topic, payload, qos, retain))
        return FakePublishInfo(rc=self.publish_rc, published=self.publish_published)

    # -- helpers for tests

    def fire_connect(self, rc=0):
        self.on_connect(self, None, {}, rc)

    def fire_disconnect(self, rc=1):
        self.on_disconnect(self, None, rc)


class FakePublisher(object):
    """Stands in for mqttclient.Publisher in scheduler tests."""

    def __init__(self, connected=True):
        self.connected = connected
        self.published = []
        self.maintain_calls = 0
        self.maintain_error = None
        self.stopped = False

    def publish(self, topic, payload, qos=None, retain=None):
        if not self.connected:
            return False
        self.published.append((topic, payload))
        return True

    def maintain(self):
        self.maintain_calls += 1
        if self.maintain_error:
            raise self.maintain_error
        return False

    def stop(self):
        self.stopped = True
