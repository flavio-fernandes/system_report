#!/usr/bin/env python3
"""system_report entry point: collect, publish, repeat.

The loop is deliberately boring. One process, one thread of control (paho runs
its own network thread), a monotonic schedule that never fires a catch-up burst
after the machine was suspended, and a watchdog ping that reflects *our* health
rather than the broker's.
"""

import argparse
import json
import signal
import sys
import threading
import time

from system_report import collectors
from system_report import config
from system_report import const
from system_report import log
from system_report import mqttclient
from system_report import sdnotify

logger = log.getLogger()


class Reporter(object):
    """Owns the schedule; knows nothing about sockets or files."""

    def __init__(self, cfg, publisher, notifier=None, clock=time.monotonic,
                 reader=collectors.read_text, now_fun=None):
        self._cfg = cfg
        self._publisher = publisher
        self._notifier = notifier
        self._clock = clock
        self._reader = reader
        self._now_fun = now_fun
        self._interval = cfg.report["interval_secs"]

        # bedclock's rule: the first report goes out one full interval in,
        # unless report_on_start asks for one as soon as we are connected.
        self._report_on_start = bool(cfg.report["report_on_start"])
        self._next_report = self._clock() if self._report_on_start \
            else self._clock() + self._interval
        self._first_report_pending = self._report_on_start
        self._consecutive_failures = 0
        self._reports = 0

    # -- one report ----------------------------------------------------------

    def collect(self):
        return collectors.collect(
            fields=self._cfg.meminfo_fields,
            reader=self._reader,
            want_uptime=self._cfg.uptime_enabled,
            want_meminfo=self._cfg.meminfo_enabled,
            hostname=self._cfg.hostname,
            now=self._now_fun() if self._now_fun else None,
        )

    def messages_for(self, snapshot):
        """Build the (topic, payload) list for a snapshot, in publish order."""
        messages = []
        if self._cfg.report["publish_plain"]:
            if self._cfg.uptime_enabled and snapshot.uptime_minutes is not None:
                messages.append((self._cfg.uptime_topic, snapshot.uptime_minutes))
            for field, value in snapshot.meminfo.items():
                messages.append((self._cfg.meminfo_topic(field), value))
        if self._cfg.report["publish_json"]:
            payload = json.dumps(collectors.to_json_dict(snapshot), separators=(",", ":"))
            messages.append((self._cfg.json_topic, payload))
        return messages

    def report_once(self):
        snapshot = self.collect()
        for problem in snapshot.errors:
            logger.warning("collection problem: %s", problem)

        messages = self.messages_for(snapshot)
        published = sum(1 for topic, payload in messages
                        if self._publisher.publish(topic, payload))
        self._reports += 1

        mem = snapshot.meminfo.get("MemAvailable")
        logger.info(
            "report #%d: published %d/%d value(s)%s%s",
            self._reports, published, len(messages),
            "" if mem is None else ", mem_available_kb={}".format(mem),
            "" if snapshot.uptime_minutes is None
            else ", uptime_minutes={}".format(snapshot.uptime_minutes),
        )
        if self._notifier:
            self._notifier.status(
                "reports={} published={}/{} connected={}".format(
                    self._reports, published, len(messages), self._publisher.connected
                )
            )
        return published

    # -- schedule ------------------------------------------------------------

    @property
    def next_report_at(self):
        return self._next_report

    def seconds_until_due(self):
        return max(0.0, self._next_report - self._clock())

    def tick(self):
        """One non-blocking iteration. False means: give up, let systemd restart us."""
        try:
            self._publisher.maintain()
            if self.seconds_until_due() <= 0:
                if self._publisher.connected:
                    self.report_once()
                    self._first_report_pending = False
                    # Schedule from now, so a slow report cannot stack up a
                    # backlog of immediately-due reports.
                    self._next_report = self._clock() + self._interval
                elif not self._first_report_pending:
                    # A normal outage: skip this slot rather than pile up.
                    logger.debug("report skipped: broker not connected")
                    self._next_report = self._clock() + self._interval
                # else: keep retrying every tick until the first connect lands.
            self._consecutive_failures = 0
            return True
        except Exception:
            self._consecutive_failures += 1
            logger.exception(
                "unexpected failure in the report loop (%d/%d consecutive)",
                self._consecutive_failures, const.MAX_CONSECUTIVE_CYCLE_FAILURES,
            )
            return self._consecutive_failures < const.MAX_CONSECUTIVE_CYCLE_FAILURES

    def sleep_secs(self, watchdog_secs=None):
        wait = min(const.LOOP_TICK_MAX_SECS, self.seconds_until_due())
        if watchdog_secs:
            wait = min(wait, max(1.0, watchdog_secs))
        if wait <= 0:
            # Due but unable to publish yet: retry without spinning.
            wait = const.PENDING_RETRY_SECS
        return wait

    def run(self, stop_event):
        watchdog_secs = self._notifier.watchdog_interval_secs if self._notifier else None
        while not stop_event.is_set():
            if not self.tick():
                return 1
            if self._notifier:
                self._notifier.watchdog()
            stop_event.wait(self.sleep_secs(watchdog_secs))
        return 0


# --- process plumbing -------------------------------------------------------


def parse_args(argv):
    parser = argparse.ArgumentParser(
        prog=const.APP_NAME,
        description="Publish Linux memory/uptime state to an MQTT broker, periodically.",
    )
    parser.add_argument(
        "config", nargs="?", default=None,
        help="path to config.yaml (default: data/config.yaml next to this checkout)",
    )
    parser.add_argument("--once", action="store_true",
                        help="publish a single report, then exit")
    parser.add_argument("--dry-run", action="store_true",
                        help="print the report that would be published; never touch the network")
    parser.add_argument("--print-config", action="store_true",
                        help="print the effective configuration (secrets redacted) and exit")
    parser.add_argument("--version", action="version",
                        version="{} {}".format(const.APP_NAME, const.VERSION))
    return parser.parse_args(argv)


def _install_signal_handlers(stop_event):
    def _handler(signum, _frame):
        logger.info("got signal %s: shutting down", signum)
        stop_event.set()

    for sig in (signal.SIGTERM, signal.SIGINT):
        signal.signal(sig, _handler)


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)

    log.initLogger()
    try:
        cfg = config.load(args.config)
    except config.ConfigError as e:
        log.log_to_console()
        logger.error("%s", e)
        return 2
    log.apply_knobs(cfg.knobs)
    cfg.log_warnings(logger)

    if args.print_config:
        print(json.dumps(cfg.redacted(), indent=2, sort_keys=True))
        return 0

    if args.dry_run:
        reporter = Reporter(cfg, publisher=_DryRunPublisher())
        snapshot = reporter.collect()
        for problem in snapshot.errors:
            logger.warning("collection problem: %s", problem)
        for topic, payload in reporter.messages_for(snapshot):
            print("{} {}".format(topic, payload))
        return 0

    logger.info(
        "%s %s starting: host=%s interval=%ss topics under %s",
        const.APP_NAME, const.VERSION, cfg.hostname,
        cfg.report["interval_secs"], cfg.prefix or "/",
    )

    notifier = sdnotify.Notifier()
    publisher = mqttclient.Publisher(cfg)
    reporter = Reporter(cfg, publisher, notifier=notifier)
    stop_event = threading.Event()
    _install_signal_handlers(stop_event)

    rc = 0
    publisher.start()
    notifier.ready()
    try:
        if args.once:
            # Wait (bounded) for the first connection, then report exactly once.
            deadline = time.monotonic() + cfg.mqtt["reconnect_max_delay_secs"]
            while not publisher.connected and time.monotonic() < deadline \
                    and not stop_event.is_set():
                stop_event.wait(0.25)
            reporter.report_once()
        else:
            rc = reporter.run(stop_event)
    finally:
        notifier.stopping()
        publisher.stop()
        notifier.close()
    logger.info("%s stopped (rc=%d)", const.APP_NAME, rc)
    return rc


class _DryRunPublisher(object):
    """Stands in for the real publisher so --dry-run cannot touch the network."""

    connected = True

    def publish(self, topic, payload, qos=None, retain=None):
        return True

    def maintain(self):
        return False


if __name__ == "__main__":
    sys.exit(main())
