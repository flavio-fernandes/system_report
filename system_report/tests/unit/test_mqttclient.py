import pytest

from system_report import mqttclient
from system_report.tests.unit.conftest import FakeClock, FakeMqttClient


@pytest.fixture
def make_publisher(make_cfg):
    """Publisher wired to a fake paho client and a clock we control."""

    def _make(overrides=None, env=None, hostname="testhost"):
        cfg = make_cfg(overrides or {}, env=env, hostname=hostname)
        clock = FakeClock()
        clients = []

        def factory(client_id, clean_session):
            client = FakeMqttClient(client_id, clean_session)
            clients.append(client)
            return client

        publisher = mqttclient.Publisher(cfg, clock=clock, client_factory=factory)
        return publisher, clients, clock, cfg

    return _make


# --- wiring -----------------------------------------------------------------


def test_start_configures_the_client_without_blocking(make_publisher):
    publisher, clients, _clock, cfg = make_publisher(
        {"mqtt": {"host": "broker.example.lan", "port": 8883, "keepalive_secs": 90}}
    )
    publisher.start()
    client = clients[0]

    assert client.connect_args == ("broker.example.lan", 8883, 90)   # connect_async
    assert client.loop_started is True
    assert client.client_id == "system_report_testhost"
    assert client.reconnect_delay == (1, 120)
    assert client.max_queued == 100
    assert publisher.connected is False        # nothing is connected synchronously
    assert client.will == (cfg.status_topic, "offline", 1, True)


def test_credentials_and_tls_are_applied_when_configured(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher({
        "mqtt": {
            "username": "bob",
            "password": "s3cret",
            "tls": {"enabled": True, "ca_certs": "/etc/ssl/ca.pem", "insecure": True},
        }
    })
    publisher.start()
    client = clients[0]
    assert client.credentials == ("bob", "s3cret")
    assert client.tls == ("/etc/ssl/ca.pem", None, None)
    assert client.tls_insecure is True


def test_no_credentials_means_no_username_call(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher()
    publisher.start()
    assert clients[0].credentials is None
    assert clients[0].tls is None


# --- connect / disconnect ---------------------------------------------------


def test_connecting_publishes_a_retained_online_status(make_publisher):
    publisher, clients, _clock, cfg = make_publisher()
    publisher.start()
    clients[0].fire_connect()

    assert publisher.connected is True
    assert clients[0].published == [(cfg.status_topic, "online", 1, True)]


def test_a_refused_connection_leaves_us_disconnected(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher()
    publisher.start()
    clients[0].fire_connect(rc=5)          # not authorized

    assert publisher.connected is False
    assert clients[0].published == []


def test_disconnect_marks_the_time_so_recovery_can_be_measured(make_publisher):
    publisher, clients, clock, _cfg = make_publisher()
    publisher.start()
    clients[0].fire_connect()
    clients[0].fire_disconnect()
    clock.advance(45)

    assert publisher.connected is False
    assert publisher.disconnected_for == pytest.approx(45)


# --- publishing -------------------------------------------------------------


def test_publish_uses_the_configured_qos_and_retain(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher({"mqtt": {"qos": 1, "retain": True}})
    publisher.start()
    clients[0].fire_connect()
    clients[0].published = []

    assert publisher.publish("/t/mem", 42) is True
    assert clients[0].published == [("/t/mem", 42, 1, True)]


def test_publishing_while_disconnected_drops_rather_than_queues(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher()
    publisher.start()      # never connected

    assert publisher.publish("/t/mem", 42) is False
    assert publisher.publish("/t/mem", 43) is False
    assert clients[0].published == []
    assert publisher.dropped == 2


def test_the_drop_counter_resets_on_reconnect(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher()
    publisher.start()
    publisher.publish("/t/mem", 42)
    assert publisher.dropped == 1

    clients[0].fire_connect()
    assert publisher.dropped == 0


def test_a_qos1_publish_that_is_never_acknowledged_is_reported_as_failed(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher({"mqtt": {"qos": 1}})
    publisher.start()
    clients[0].fire_connect()
    clients[0].publish_published = False

    assert publisher.publish("/t/mem", 42) is False


def test_a_client_side_publish_error_is_contained(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher()
    publisher.start()
    clients[0].fire_connect()

    def boom(*args, **kwargs):
        raise RuntimeError("socket gone")

    clients[0].publish = boom
    assert publisher.publish("/t/mem", 42) is False      # no exception escapes


# --- self healing -----------------------------------------------------------


def test_a_long_outage_rebuilds_the_client(make_publisher):
    publisher, clients, clock, _cfg = make_publisher(
        {"mqtt": {"recreate_client_after_secs": 900}}
    )
    publisher.start()
    clients[0].fire_connect()
    clients[0].fire_disconnect()

    clock.advance(899)
    assert publisher.maintain() is False
    assert len(clients) == 1

    clock.advance(2)
    assert publisher.maintain() is True
    assert len(clients) == 2                 # a brand new client took over
    assert clients[0].loop_stopped is True
    assert clients[1].loop_started is True


def test_maintain_is_a_noop_while_connected_or_when_disabled(make_publisher):
    publisher, clients, clock, _cfg = make_publisher(
        {"mqtt": {"recreate_client_after_secs": 0}}
    )
    publisher.start()
    clients[0].fire_connect()
    clock.advance(10000)
    assert publisher.maintain() is False

    clients[0].fire_disconnect()
    clock.advance(10000)
    assert publisher.maintain() is False      # disabled by config
    assert len(clients) == 1


# --- shutdown ---------------------------------------------------------------


def test_stop_says_goodbye_then_disconnects(make_publisher):
    publisher, clients, _clock, cfg = make_publisher()
    publisher.start()
    clients[0].fire_connect()
    clients[0].published = []

    publisher.stop()
    assert clients[0].published == [(cfg.status_topic, "offline", 1, True)]
    assert clients[0].disconnected is True
    assert clients[0].loop_stopped is True


def test_stop_while_disconnected_is_quiet(make_publisher):
    publisher, clients, _clock, _cfg = make_publisher()
    publisher.start()
    publisher.stop()
    assert clients[0].published == []
    assert clients[0].loop_stopped is True


def test_reason_code_helper_accepts_both_paho_apis():
    class V2ReasonCode(object):
        def __init__(self, failure):
            self.is_failure = failure

    assert mqttclient._rc_is_success(0) is True
    assert mqttclient._rc_is_success(5) is False
    assert mqttclient._rc_is_success(V2ReasonCode(False)) is True
    assert mqttclient._rc_is_success(V2ReasonCode(True)) is False
