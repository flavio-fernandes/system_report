//! Best-effort systemd readiness and watchdog notifications.
//!
//! Configuration is captured at construction, never changed in the process environment.
//! `sd-notify` supplies protocol states; the transport preserves the Python client's
//! exact datagrams (no trailing newline), explicit environment, and retry behavior.
use sd_notify::NotifyState;
use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixDatagram;

#[derive(Debug)]
pub struct Notifier {
    address: Option<String>,
    watchdog_usec: Option<u64>,
    socket: Option<UnixDatagram>,
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new()
    }
}

impl Notifier {
    /// Capture the current systemd environment without unsetting any variables.
    pub fn new() -> Self {
        let env = ["NOTIFY_SOCKET", "WATCHDOG_PID", "WATCHDOG_USEC"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
            .collect();
        Self::from_env(&env)
    }

    /// Explicit environment for callers and race-free tests.
    pub fn from_env(env: &HashMap<String, String>) -> Self {
        let address = env.get("NOTIFY_SOCKET").filter(|s| !s.is_empty()).map(|s| {
            s.strip_prefix('@')
                .map_or_else(|| s.clone(), |name| format!("\0{name}"))
        });
        let pid = std::process::id().to_string();
        let matches_pid = env
            .get("WATCHDOG_PID")
            .is_none_or(|value| value.is_empty() || value == &pid);
        let watchdog_usec = matches_pid
            .then(|| env.get("WATCHDOG_USEC")?.trim().parse::<u64>().ok())
            .flatten()
            .filter(|value| *value > 0);
        Self {
            address,
            watchdog_usec,
            socket: None,
        }
    }

    pub fn enabled(&self) -> bool {
        self.address.is_some()
    }

    /// Resolved address: a leading `@` is represented as a leading NUL.
    pub fn address(&self) -> Option<&str> {
        self.address.as_deref()
    }

    /// Half the watchdog deadline, including fractional microseconds.
    pub fn watchdog_interval_secs(&self) -> Option<f64> {
        self.watchdog_usec
            .map(|usec| usec as f64 / 2.0 / 1_000_000.0)
    }

    /// Send one UTF-8 datagram. Errors are logged and never propagated.
    pub fn notify(&mut self, state: &str) -> bool {
        if !self.enabled() {
            return false;
        }
        match self.send(state.as_bytes()) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("sd_notify {state:?} failed: {error}");
                self.close();
                false
            }
        }
    }

    fn send(&mut self, message: &[u8]) -> io::Result<()> {
        let address = self.address.as_deref().expect("checked by notify");
        if self.socket.is_none() {
            self.socket = Some(UnixDatagram::unbound()?);
        }
        let socket = self.socket.as_ref().expect("created above");
        let sent = if let Some(name) = address.strip_prefix('\0') {
            #[cfg(target_os = "linux")]
            {
                use std::os::linux::net::SocketAddrExt;
                use std::os::unix::net::SocketAddr;
                socket.send_to_addr(message, &SocketAddr::from_abstract_name(name.as_bytes())?)?
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = name;
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "abstract Unix socket",
                ));
            }
        } else {
            socket.send_to(message, address)?
        };
        if sent != message.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "incomplete notification",
            ));
        }
        Ok(())
    }

    pub fn ready(&mut self) -> bool {
        self.notify(&NotifyState::Ready.to_string())
    }

    pub fn watchdog(&mut self) -> bool {
        self.watchdog_usec.is_some() && self.notify(&NotifyState::Watchdog.to_string())
    }

    pub fn status(&mut self, text: &str) -> bool {
        self.notify(&NotifyState::Status(text).to_string())
    }

    pub fn stopping(&mut self) -> bool {
        self.notify(&NotifyState::Stopping.to_string())
    }

    /// Release the socket; subsequent notifications may open a fresh one.
    /// Dropping the notifier also releases it automatically.
    pub fn close(&mut self) {
        self.socket = None;
    }
}
