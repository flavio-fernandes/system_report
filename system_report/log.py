#!/usr/bin/env python3
"""Logging setup: journal/syslog when available, stdout otherwise."""

import logging
from logging.handlers import SysLogHandler
from os import path

from system_report import const

_LOG_SOCKETS = ("/run/systemd/journal/syslog", "/var/run/syslog", "/dev/log")


def getLogger():
    return logging.getLogger(const.APP_NAME)


def _log_handler_address(files=_LOG_SOCKETS):
    try:
        return next(f for f in files if path.exists(f))
    except StopIteration:
        return None


def initLogger(testing=False):
    logger = getLogger()
    logger.setLevel(logging.INFO)

    fmt = "%(asctime)s [{}] %(module)12s:%(lineno)-d %(levelname)-8s %(message)s".format(
        const.APP_NAME
    )
    formatter = logging.Formatter(fmt)

    address = _log_handler_address()
    if address:
        try:
            handler = SysLogHandler(address=address, facility=SysLogHandler.LOG_DAEMON)
        except (OSError, IOError):
            handler = logging.StreamHandler()
    else:
        # Under systemd, stdout/stderr already land in the journal.
        handler = logging.StreamHandler()
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
