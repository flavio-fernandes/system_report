#!/usr/bin/env python3
"""MQTT publishing written on the assumption that the broker will go away.

What that means concretely:

* the connection is never established synchronously, so a broker that is down
  at boot (or forever) cannot block or crash the reporter;
* paho's own network thread reconnects with exponential backoff, and if it is
  still not connected after ``recreate_client_after_secs`` the client object is
  rebuilt from scratch -- the belt to paho's braces;
* reports produced while disconnected are **dropped, not queued**. A reporter
  whose memory grows during an outage would be a poor tool for finding leaks;
* a retained last-will tells subscribers this host went away without notice.
"""

import threading
import time

import paho.mqtt.client as mqtt

from system_report import const
from system_report import log

logger = log.getLogger()


def _rc_is_success(rc):
    """True for a successful reason code under either paho callback API."""
    if rc is None:
        return True
    is_failure = getattr(rc, "is_failure", None)
    if is_failure is not None:
        return not is_failure
    try:
        return int(rc) == 0
    except (TypeError, ValueError):
        return False


def new_paho_client(client_id, clean_session):
    """paho 1.x and 2.x differ in the constructor; hide that here."""
    if hasattr(mqtt, "CallbackAPIVersion"):
        return mqtt.Client(
            mqtt.CallbackAPIVersion.VERSION2, client_id=client_id, clean_session=clean_session
        )
    return mqtt.Client(client_id=client_id, clean_session=clean_session)


class Publisher(object):
    def __init__(self, cfg, clock=time.monotonic, client_factory=None):
        self._cfg = cfg
        self._clock = clock
        self._factory = client_factory or new_paho_client
        self._lock = threading.Lock()
        self._client = None
        self._connected = False
        self._disconnected_since = clock()
        self._dropped = 0
        self._last_drop_log = None

    # -- state ---------------------------------------------------------------

    @property
    def connected(self):
        return self._connected

    @property
    def dropped(self):
        return self._dropped

    @property
    def disconnected_for(self):
        if self._connected or self._disconnected_since is None:
            return 0.0
        return self._clock() - self._disconnected_since

    # -- lifecycle -----------------------------------------------------------

    def start(self):
        with self._lock:
            self._build_locked()

    def stop(self):
        """Say goodbye properly: publish 'offline', then disconnect."""
        with self._lock:
            if self._client is None:
                return
            if self._connected:
                self.publish(
                    self._cfg.status_topic,
                    self._cfg.topics["payload_offline"],
                    qos=1,
                    retain=bool(self._cfg.topics["status_retain"]),
                )
            self._teardown_locked()

    def maintain(self):
        """Rebuild the client if the connection has been gone for too long."""
        after = self._cfg.mqtt["recreate_client_after_secs"]
        if after <= 0 or self._connected or self._client is None:
            return False
        gone_for = self.disconnected_for
        if gone_for < after:
            return False
        logger.warning(
            "no broker connection for %.0fs: rebuilding the mqtt client", gone_for
        )
        with self._lock:
            self._teardown_locked()
            self._build_locked()
        return True

    # -- publishing ----------------------------------------------------------

    def publish(self, topic, payload, qos=None, retain=None):
        client = self._client
        if client is None or not self._connected:
            self._note_drop(topic)
            return False

        mqtt_cfg = self._cfg.mqtt
        qos = mqtt_cfg["qos"] if qos is None else qos
        retain = mqtt_cfg["retain"] if retain is None else retain
        timeout = mqtt_cfg["publish_timeout_secs"]
        try:
            info = client.publish(topic, payload, qos=qos, retain=retain)
            rc = getattr(info, "rc", 0)
            if not _rc_is_success(rc):
                logger.warning("client refused to publish %s: rc=%s", topic, rc)
                return False
            if qos > 0:
                info.wait_for_publish(timeout)
                if not info.is_published():
                    logger.warning("publish of %s timed out after %.1fs", topic, timeout)
                    return False
        except Exception as e:
            logger.error("failed to publish %s: %s", topic, e)
            return False
        logger.debug("published %s %s", topic, payload)
        return True

    # -- internals -----------------------------------------------------------

    def _build_locked(self):
        cfg = self._cfg
        mqtt_cfg = cfg.mqtt
        tls_cfg = mqtt_cfg["tls"]

        client = self._factory(cfg.client_id, bool(mqtt_cfg["clean_session"]))
        if mqtt_cfg.get("username"):
            client.username_pw_set(mqtt_cfg["username"], cfg.password)
        if tls_cfg["enabled"]:
            client.tls_set(
                ca_certs=tls_cfg["ca_certs"],
                certfile=tls_cfg["certfile"],
                keyfile=tls_cfg["keyfile"],
            )
            if tls_cfg["insecure"]:
                client.tls_insecure_set(True)
        client.reconnect_delay_set(
            min_delay=mqtt_cfg["reconnect_min_delay_secs"],
            max_delay=mqtt_cfg["reconnect_max_delay_secs"],
        )
        client.max_queued_messages_set(mqtt_cfg["max_queued_messages"])
        client.will_set(
            cfg.status_topic,
            cfg.topics["payload_offline"],
            qos=1,
            retain=bool(cfg.topics["status_retain"]),
        )
        client.on_connect = self._on_connect
        client.on_disconnect = self._on_disconnect

        self._client = client
        self._connected = False
        self._disconnected_since = self._clock()

        logger.info(
            "connecting to mqtt broker %s:%s as client_id %s (tls=%s, auth=%s)",
            mqtt_cfg["host"], mqtt_cfg["port"], cfg.client_id,
            "on" if tls_cfg["enabled"] else "off",
            "on" if mqtt_cfg.get("username") else "off",
        )
        # connect_async + loop_start: the network thread owns the (re)connecting,
        # including retrying the very first attempt.
        client.connect_async(mqtt_cfg["host"], mqtt_cfg["port"],
                             keepalive=mqtt_cfg["keepalive_secs"])
        client.loop_start()

    def _teardown_locked(self):
        client, self._client = self._client, None
        self._connected = False
        if client is None:
            return
        for step in ("disconnect", "loop_stop"):
            try:
                getattr(client, step)()
            except Exception as e:
                logger.debug("mqtt client %s() raised: %s", step, e)

    def _note_drop(self, topic):
        self._dropped += 1
        now = self._clock()
        if (self._last_drop_log is None
                or (now - self._last_drop_log) >= const.DROP_LOG_INTERVAL_SECS):
            self._last_drop_log = now
            logger.warning(
                "broker not connected (%.0fs): dropped %d message(s) so far, latest %s",
                self.disconnected_for, self._dropped, topic,
            )

    # -- paho callbacks (these run on paho's network thread) -----------------

    def _on_connect(self, client, userdata, flags, rc, *args):
        if not _rc_is_success(rc):
            logger.warning(
                "broker %s:%s refused the connection: %s",
                self._cfg.mqtt["host"], self._cfg.mqtt["port"], rc,
            )
            return
        self._connected = True
        self._disconnected_since = None
        dropped, self._dropped = self._dropped, 0
        self._last_drop_log = None
        logger.info(
            "connected to mqtt broker %s:%s%s",
            self._cfg.mqtt["host"], self._cfg.mqtt["port"],
            " ({} message(s) were dropped while away)".format(dropped) if dropped else "",
        )
        try:
            client.publish(
                self._cfg.status_topic,
                self._cfg.topics["payload_online"],
                qos=1,
                retain=bool(self._cfg.topics["status_retain"]),
            )
        except Exception as e:
            logger.error("failed to publish the online status: %s", e)

    def _on_disconnect(self, client, userdata, *args):
        was_connected = self._connected
        self._connected = False
        self._disconnected_since = self._clock()
        if was_connected:
            logger.warning("disconnected from the mqtt broker (%s); reconnecting", args)
