"""Where do log records actually go?

This is worth testing rather than eyeballing: the first version of this file
sent records to /run/systemd/journal/syslog, which is journald's socket for
forwarding *out* to rsyslog. Everything looked fine -- the service logged, the
lines appeared in /var/log/syslog -- but `journalctl -u system_report.service`,
which the README and bin/tail_log.sh both point at, showed nothing but systemd's
own start and stop messages.
"""

import logging
import os
import shutil
import socket
import tempfile
from logging.handlers import SysLogHandler

import pytest

from system_report import log


@pytest.fixture
def syslog_socket():
    """A bound unix datagram socket standing in for /dev/log."""
    parent = tempfile.gettempdir()
    if len(parent) > 40:                     # macOS /var/folders/... is long
        parent = "/tmp"
    directory = tempfile.mkdtemp(prefix="sysrep-log-", dir=parent)
    path = os.path.join(directory, "log.sock")
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sock.bind(path)
    try:
        yield path
    finally:
        sock.close()
        shutil.rmtree(directory, ignore_errors=True)


def test_under_systemd_we_log_to_stdout_so_the_journal_attributes_it(syslog_socket):
    # JOURNAL_STREAM set means systemd is already capturing stdout; a syslog
    # socket must not win over it, even when one exists.
    handler = log.build_handler(env={"JOURNAL_STREAM": "9:1328863"},
                                files=(syslog_socket,))
    assert isinstance(handler, logging.StreamHandler)
    assert not isinstance(handler, SysLogHandler)


def test_without_systemd_we_use_the_syslog_socket(syslog_socket):
    handler = log.build_handler(env={}, files=(syslog_socket,))
    try:
        assert isinstance(handler, SysLogHandler)
    finally:
        handler.close()


def test_the_journald_forwarding_socket_is_not_in_the_default_list():
    # Regression guard for the original bug: records sent here reach rsyslog
    # and never the journal.
    assert "/run/systemd/journal/syslog" not in log._LOG_SOCKETS
    assert "/dev/log" in log._LOG_SOCKETS


def test_with_no_socket_at_all_we_still_log(tmp_path):
    handler = log.build_handler(env={}, files=(os.path.join(str(tmp_path), "nope"),))
    assert isinstance(handler, logging.StreamHandler)


def test_a_path_that_is_not_a_socket_is_not_mistaken_for_one(tmp_path):
    # Existence is not enough: SysLogHandler constructs happily against a
    # regular file and only fails later, once per record, inside emit().
    not_a_socket = os.path.join(str(tmp_path), "not-a-socket")
    with open(not_a_socket, "w") as handle:
        handle.write("")
    assert log._log_handler_address((not_a_socket,)) is None
    handler = log.build_handler(env={}, files=(not_a_socket,))
    assert isinstance(handler, logging.StreamHandler)
    assert not isinstance(handler, SysLogHandler)
