import os
import socket

import pytest

from system_report import sdnotify


@pytest.fixture
def listener(tmp_path):
    """A real unix datagram socket standing in for systemd's."""
    path = os.path.join(str(tmp_path), "notify.sock")
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


def test_abstract_namespace_addresses_are_understood(tmp_path):
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


def test_a_dead_socket_does_not_raise(tmp_path):
    missing = os.path.join(str(tmp_path), "gone.sock")
    notifier = sdnotify.Notifier(env={"NOTIFY_SOCKET": missing})
    assert notifier.ready() is False       # logged, not raised
