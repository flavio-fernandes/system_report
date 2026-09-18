import os
import shutil
import socket
import sys
import tempfile

import pytest

from system_report import sdnotify

# A unix socket address is capped by sun_path: 104 bytes on macOS/BSD, 108 on
# Linux. Stay well clear of the smaller one.
SUN_PATH_BUDGET = 100

LINUX_ONLY = pytest.mark.skipif(
    not sys.platform.startswith("linux"),
    reason="abstract-namespace unix sockets exist only on Linux",
)


@pytest.fixture
def sock_dir():
    """A scratch directory whose path is short enough to hold a socket.

    Deliberately not pytest's tmp_path: on macOS that expands to something like
    /private/var/folders/<hash>/T/pytest-of-<user>/pytest-N/<test-name>0/, and
    this file's test names are long, so the socket path overruns sun_path and
    bind() fails before the test does anything.
    """
    parent = tempfile.gettempdir()
    if len(parent) > 40:                     # macOS /var/folders/... is long
        parent = "/tmp"
    path = tempfile.mkdtemp(prefix="sysrep-", dir=parent)
    try:
        yield path
    finally:
        shutil.rmtree(path, ignore_errors=True)


def socket_path(directory, name="notify.sock"):
    """Join, and fail loudly rather than with an obscure bind() error."""
    path = os.path.join(directory, name)
    assert len(path) < SUN_PATH_BUDGET, "socket path too long for sun_path: " + path
    return path


@pytest.fixture
def listener(sock_dir):
    """A real unix datagram socket standing in for systemd's."""
    path = socket_path(sock_dir)
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sock.bind(path)
    sock.settimeout(2)
    try:
        yield path, sock
    finally:
        sock.close()


def test_notifications_reach_the_socket(listener):
    path, sock = listener
    notifier = sdnotify.Notifier(env={"NOTIFY_SOCKET": path})

    assert notifier.enabled is True
    assert notifier.ready() is True
    assert sock.recv(64) == b"READY=1"

    notifier.status("all good")
    assert sock.recv(64) == b"STATUS=all good"

    notifier.stopping()
    assert sock.recv(64) == b"STOPPING=1"
    notifier.close()


def test_without_the_env_var_everything_is_a_noop():
    notifier = sdnotify.Notifier(env={})
    assert notifier.enabled is False
    assert notifier.ready() is False
    assert notifier.watchdog() is False
    assert notifier.watchdog_interval_secs is None


def test_the_watchdog_is_petted_only_when_systemd_asked_for_it(listener):
    path, sock = listener

    silent = sdnotify.Notifier(env={"NOTIFY_SOCKET": path})
    assert silent.watchdog() is False
    assert silent.watchdog_interval_secs is None

    armed = sdnotify.Notifier(env={"NOTIFY_SOCKET": path, "WATCHDOG_USEC": "180000000"})
    assert armed.watchdog_interval_secs == pytest.approx(90.0)   # half the deadline
    assert armed.watchdog() is True
    assert sock.recv(64) == b"WATCHDOG=1"


def test_a_watchdog_meant_for_another_pid_is_ignored(listener):
    path, _sock = listener
    env = {"NOTIFY_SOCKET": path, "WATCHDOG_USEC": "180000000",
           "WATCHDOG_PID": str(os.getpid() + 1)}
    assert sdnotify.Notifier(env=env).watchdog_interval_secs is None


def test_a_garbled_watchdog_value_is_ignored(listener):
    path, _sock = listener
    env = {"NOTIFY_SOCKET": path, "WATCHDOG_USEC": "not-a-number"}
    assert sdnotify.Notifier(env=env).watchdog_interval_secs is None


def test_an_at_prefixed_address_is_translated_to_the_abstract_namespace():
    # systemd spells an abstract-namespace address with a leading '@'; the
    # kernel spells it with a leading NUL. That translation is pure string
    # work, so it is tested everywhere -- including on platforms with no
    # abstract namespace, where the round-trip test below cannot run and this
    # line would otherwise go uncovered.
    assert sdnotify.Notifier(env={"NOTIFY_SOCKET": "@sysrep"}).address == "\0sysrep"
    assert sdnotify.Notifier(env={"NOTIFY_SOCKET": "/run/x.sock"}).address == "/run/x.sock"
    assert sdnotify.Notifier(env={}).address is None


@LINUX_ONLY
def test_abstract_namespace_addresses_are_understood():
    name = "\0system-report-test-{}".format(os.getpid())
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sock.bind(name)
    sock.settimeout(2)
    try:
        notifier = sdnotify.Notifier(env={"NOTIFY_SOCKET": "@" + name[1:]})
        assert notifier.ready() is True
        assert sock.recv(64) == b"READY=1"
    finally:
        sock.close()


def test_a_dead_socket_does_not_raise(sock_dir):
    missing = socket_path(sock_dir, "gone.sock")
    notifier = sdnotify.Notifier(env={"NOTIFY_SOCKET": missing})
    assert notifier.ready() is False       # logged, not raised
