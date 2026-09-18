import json
import threading

import pytest

from system_report import const
from system_report import main
from system_report.tests.unit.conftest import FakeClock, FakePublisher


@pytest.fixture
def make_reporter(make_cfg, proc_reader):
    def _make(overrides=None, connected=True, reader=None, hostname="testhost"):
        cfg = make_cfg(overrides or {}, hostname=hostname)
        publisher = FakePublisher(connected=connected)
        clock = FakeClock()
        reporter = main.Reporter(cfg, publisher, clock=clock,
                                 reader=reader or proc_reader())
        return reporter, publisher, clock, cfg

    return _make


DEFAULT = {"report": {"interval_secs": 600}}


# --- what a report contains -------------------------------------------------


def test_a_report_publishes_plain_values_and_one_json_document(make_reporter):
    reporter, publisher, _clock, cfg = make_reporter(
        {"metrics": {"meminfo": {"fields": ["MemAvailable", "Slab"]}}}
    )
    reporter.report_once()

    topics = [topic for topic, _payload in publisher.published]
    assert topics == [
        "/testhost/oper_uptime_minutes",
        "/testhost/oper_state/mem_available_kb",
        "/testhost/oper_state/slab_kb",
        "/testhost/oper_state/json",
    ]
    assert dict(publisher.published)["/testhost/oper_state/mem_available_kb"] == 457392
    payload = json.loads(dict(publisher.published)[cfg.json_topic])
    assert payload["mem_available_kb"] == 457392
    assert payload["uptime_minutes"] == 1552


def test_publish_plain_can_be_turned_off(make_reporter):
    reporter, publisher, _clock, cfg = make_reporter({"report": {"publish_plain": False}})
    reporter.report_once()
    assert [t for t, _ in publisher.published] == [cfg.json_topic]


def test_publish_json_can_be_turned_off(make_reporter):
    reporter, publisher, _clock, cfg = make_reporter({"report": {"publish_json": False}})
    reporter.report_once()
    assert cfg.json_topic not in [t for t, _ in publisher.published]


def test_a_disabled_metric_is_not_published(make_reporter):
    reporter, publisher, _clock, _cfg = make_reporter(
        {"metrics": {"uptime_minutes": {"enabled": False},
                     "meminfo": {"fields": ["MemAvailable"]}}}
    )
    reporter.report_once()
    assert "/testhost/oper_uptime_minutes" not in [t for t, _ in publisher.published]


def test_a_collection_failure_still_publishes_what_worked(make_reporter, proc_reader):
    reader = proc_reader({"/proc/uptime": IOError("nope")})
    reporter, publisher, _clock, _cfg = make_reporter(
        {"metrics": {"meminfo": {"fields": ["MemAvailable"]}}}, reader=reader
    )
    reporter.report_once()
    topics = [t for t, _ in publisher.published]
    assert "/testhost/oper_state/mem_available_kb" in topics
    assert "/testhost/oper_uptime_minutes" not in topics


# --- the schedule -----------------------------------------------------------


def test_report_on_start_fires_immediately_then_once_per_interval(make_reporter):
    reporter, publisher, clock, _cfg = make_reporter(DEFAULT)
    reporter.tick()
    assert len(publisher.published) > 0

    published = len(publisher.published)
    clock.advance(599)
    reporter.tick()
    assert len(publisher.published) == published      # not due yet

    clock.advance(2)
    reporter.tick()
    assert len(publisher.published) > published


def test_without_report_on_start_the_first_report_waits_one_interval(make_reporter):
    reporter, publisher, clock, _cfg = make_reporter(
        {"report": {"interval_secs": 600, "report_on_start": False}}
    )
    reporter.tick()
    assert publisher.published == []

    clock.advance(600)
    reporter.tick()
    assert publisher.published != []


def test_a_long_stall_does_not_produce_a_catch_up_burst(make_reporter):
    reporter, publisher, clock, _cfg = make_reporter(DEFAULT)
    reporter.tick()                      # first report
    reports = len(publisher.published)

    clock.advance(600 * 10)              # laptop slept, or the box was frozen
    reporter.tick()
    per_report = reports
    assert len(publisher.published) == reports + per_report   # exactly one more


def test_the_first_report_waits_for_the_broker_instead_of_being_skipped(make_reporter):
    reporter, publisher, clock, _cfg = make_reporter(DEFAULT, connected=False)
    for _ in range(3):
        reporter.tick()
        clock.advance(1)
    assert publisher.published == []

    publisher.connected = True
    reporter.tick()
    assert publisher.published != []


def test_a_later_outage_skips_the_slot_rather_than_stacking(make_reporter):
    reporter, publisher, clock, _cfg = make_reporter(DEFAULT)
    reporter.tick()                      # first report, connected
    reports = len(publisher.published)

    publisher.connected = False
    clock.advance(600)
    reporter.tick()                      # due, but offline: skipped
    assert len(publisher.published) == reports

    # the slot moved on rather than firing the moment we reconnect
    publisher.connected = True
    clock.advance(1)
    reporter.tick()
    assert len(publisher.published) == reports

    clock.advance(600)
    reporter.tick()
    assert len(publisher.published) > reports


# --- failure handling -------------------------------------------------------


def test_a_failing_tick_is_survived_then_finally_gives_up(make_reporter):
    reporter, publisher, _clock, _cfg = make_reporter(DEFAULT)
    publisher.maintain_error = RuntimeError("boom")

    for _ in range(const.MAX_CONSECUTIVE_CYCLE_FAILURES - 1):
        assert reporter.tick() is True
    assert reporter.tick() is False       # systemd should restart us now


def test_the_failure_counter_resets_after_a_good_tick(make_reporter):
    reporter, publisher, _clock, _cfg = make_reporter(DEFAULT)
    publisher.maintain_error = RuntimeError("boom")
    reporter.tick()
    publisher.maintain_error = None
    reporter.tick()
    publisher.maintain_error = RuntimeError("boom")

    for _ in range(const.MAX_CONSECUTIVE_CYCLE_FAILURES - 1):
        assert reporter.tick() is True


# --- sleeping ---------------------------------------------------------------


def test_sleep_is_bounded_and_never_spins(make_reporter):
    reporter, publisher, clock, _cfg = make_reporter(DEFAULT, connected=False)
    # due right away but offline: must not busy-loop
    assert reporter.sleep_secs() == const.PENDING_RETRY_SECS

    publisher.connected = True
    reporter.tick()
    assert reporter.sleep_secs() == const.LOOP_TICK_MAX_SECS

    clock.advance(600 - 2)
    assert reporter.sleep_secs() == pytest.approx(2)


def test_a_short_watchdog_deadline_shortens_the_sleep(make_reporter):
    reporter, _publisher, _clock, _cfg = make_reporter(DEFAULT)
    reporter.tick()
    assert reporter.sleep_secs(watchdog_secs=1.5) == pytest.approx(1.5)


def test_run_returns_when_the_stop_event_is_set(make_reporter):
    reporter, publisher, _clock, _cfg = make_reporter(DEFAULT)
    stop = threading.Event()
    stop.set()
    assert reporter.run(stop) == 0
    assert publisher.published == []      # stopped before doing any work


class _NoSleepEvent(threading.Event):
    """Never set, but wait() returns at once so run() does not really sleep."""

    def wait(self, timeout=None):
        return True


def test_run_gives_up_with_a_nonzero_code_after_repeated_failures(make_reporter):
    reporter, publisher, _clock, _cfg = make_reporter(DEFAULT)
    publisher.maintain_error = RuntimeError("boom")

    assert reporter.run(_NoSleepEvent()) == 1
    assert publisher.maintain_calls == const.MAX_CONSECUTIVE_CYCLE_FAILURES
