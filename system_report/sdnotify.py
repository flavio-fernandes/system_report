#!/usr/bin/env python3
"""Minimal sd_notify client (stdlib only).

systemd hands a unix datagram socket to the service in $NOTIFY_SOCKET; writing
"READY=1" to it satisfies Type=notify, and "WATCHDOG=1" pets WatchdogSec. That
is the whole protocol we need, so there is no reason to take a dependency for
it. Outside systemd (a manual foreground run) every call is a no-op.
"""

import os
import socket

from system_report import log

logger = log.getLogger()


class Notifier(object):
    def __init__(self, env=None):
        env = os.environ if env is None else env
        self._address = env.get("NOTIFY_SOCKET") or None
        # An address starting with '@' means the abstract namespace.
        if self._address and self._address.startswith("@"):
            self._address = "\0" + self._address[1:]
        self._socket = None

        self._watchdog_usec = 0
        watchdog_pid = env.get("WATCHDOG_PID")
        if watchdog_pid in (None, "", str(os.getpid())):
            try:
                self._watchdog_usec = int(env.get("WATCHDOG_USEC") or 0)
            except ValueError:
                self._watchdog_usec = 0

    @property
    def enabled(self):
        return bool(self._address)

    @property
    def address(self):
        """The resolved socket address, or None.

        systemd may hand us an abstract-namespace address, which it spells
        with a leading '@' and the kernel spells with a leading NUL. That
        translation is pure string work, so exposing the result keeps it
        testable on platforms that have no abstract namespace at all.
        """
        return self._address

    @property
    def watchdog_interval_secs(self):
        """How often to pet the watchdog: half the systemd deadline."""
        if self._watchdog_usec <= 0:
            return None
        return self._watchdog_usec / 2.0 / 1000000.0

    def notify(self, state):
        if not self._address:
            return False
        try:
            if self._socket is None:
                self._socket = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
            self._socket.sendto(state.encode("utf-8"), self._address)
            return True
        except (OSError, IOError) as e:
            # Never let telemetry about liveness break liveness.
            logger.warning("sd_notify %r failed: %s", state, e)
            self._close()
            return False

    def ready(self):
        return self.notify("READY=1")

    def watchdog(self):
        if self._watchdog_usec <= 0:
            return False
        return self.notify("WATCHDOG=1")

    def status(self, text):
        return self.notify("STATUS={}".format(text))

    def stopping(self):
        return self.notify("STOPPING=1")

    def _close(self):
        if self._socket is not None:
            try:
                self._socket.close()
            except (OSError, IOError):
                pass
            self._socket = None

    def close(self):
        self._close()
