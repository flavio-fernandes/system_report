#!/usr/bin/env python3
"""Logging setup: journal/syslog when available, stdout otherwise."""

import logging
import os
import stat
from logging.handlers import SysLogHandler

from system_report import const

# /dev/log is journald's own socket (and a classic syslogd's). Note what is
# NOT here: /run/systemd/journal/syslog, which is the socket journald uses to
# forward *out* to rsyslog -- writing there bypasses the journal, so the lines
# never appear under `journalctl -u`, which is where this project's own docs
# tell people to look.
_LOG_SOCKETS = ("/dev/log", "/var/run/syslog")


def getLogger():
    return logging.getLogger(const.APP_NAME)


def _log_handler_address(files=_LOG_SOCKETS):
    """First entry that is genuinely a socket.

    Checking the type rather than mere existence matters: SysLogHandler will
    happily construct against a regular file and only fail later, once per log
    record, at emit() time.
    """
    for candidate in files:
        try:
            if stat.S_ISSOCK(os.stat(candidate).st_mode):
                return candidate
        except OSError:
            continue
    return None


def build_handler(env=None, files=_LOG_SOCKETS):
    """Pick where log records should go, given the environment we woke up in.

    systemd sets JOURNAL_STREAM when it is already capturing this process's
    stdout. In that case stdout is the right destination: the journal records
    each line against the unit, so `journalctl -u` and bin/tail_log.sh show
    them. Logging to a syslog socket as well would only duplicate every line.
    """
    env = os.environ if env is None else env
    if env.get("JOURNAL_STREAM"):
        return logging.StreamHandler()

    address = _log_handler_address(files)
    if address:
        try:
            return SysLogHandler(address=address, facility=SysLogHandler.LOG_DAEMON)
        except (OSError, IOError):
            return logging.StreamHandler()
    return logging.StreamHandler()


def initLogger(testing=False, env=None):
    logger = getLogger()
    logger.setLevel(logging.INFO)

    fmt = "%(asctime)s [{}] %(module)12s:%(lineno)-d %(levelname)-8s %(message)s".format(
        const.APP_NAME
    )
    formatter = logging.Formatter(fmt)

    handler = build_handler(env=env)
    handler.setFormatter(formatter)
    logger.addHandler(handler)

    if testing:
        log_to_console()
        set_log_level_debug()


def log_to_console():
    console = logging.StreamHandler()
    console.setFormatter(
        logging.Formatter("%(asctime)s %(module)12s:%(lineno)-d %(levelname)-8s %(message)s")
    )
    getLogger().addHandler(console)


def set_log_level_debug():
    getLogger().setLevel(logging.DEBUG)


def apply_knobs(knobs):
    """Honor the log_to_console / log_level_debug knobs from the config."""
    if not isinstance(knobs, dict):
        return
    if knobs.get("log_to_console"):
        log_to_console()
    if knobs.get("log_level_debug"):
        set_log_level_debug()
