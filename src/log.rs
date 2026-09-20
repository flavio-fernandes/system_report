//! Journal-aware logging. Like Python StreamHandler, console output uses stderr.
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::{
    fs::FileTypeExt,
    net::{UnixDatagram, UnixStream},
};
use std::path::{Path, PathBuf};

pub const LOG_SOCKETS: &[&str] = &["/dev/log", "/var/run/syslog"];

#[derive(Debug)]
pub enum Handler {
    Console,
    #[cfg(unix)]
    Datagram(UnixDatagram),
    #[cfg(unix)]
    Stream(UnixStream),
}
pub fn log_handler_address(files: &[&Path]) -> Option<PathBuf> {
    #[cfg(unix)]
    for path in files {
        if std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket()) {
            return Some(path.to_path_buf());
        }
    }
    #[cfg(not(unix))]
    let _ = files;
    None
}
pub fn build_handler(journal_stream: Option<&str>, files: &[&Path]) -> Handler {
    if journal_stream.is_some_and(|s| !s.is_empty()) {
        return Handler::Console;
    }
    #[cfg(unix)]
    if let Some(path) = log_handler_address(files) {
        if let Ok(socket) = UnixDatagram::unbound()
            && socket.connect(&path).is_ok()
            && socket
                .set_write_timeout(Some(std::time::Duration::from_secs(1)))
                .is_ok()
        {
            return Handler::Datagram(socket);
        }
        if let Ok(socket) = UnixStream::connect(&path)
            && socket
                .set_write_timeout(Some(std::time::Duration::from_secs(1)))
                .is_ok()
        {
            return Handler::Stream(socket);
        }
    }
    #[cfg(not(unix))]
    let _ = files;
    Handler::Console
}
impl Handler {
    pub fn emit(&mut self, level: ::log::Level, message: &str) -> io::Result<()> {
        let severity = match level {
            ::log::Level::Error => 3,
            ::log::Level::Warn => 4,
            ::log::Level::Info => 6,
            _ => 7,
        };
        // LOG_DAEMON (3 << 3), matching Python's SysLogHandler including its NUL.
        let packet = format!("<{}>{message}\0", 24 + severity);
        match self {
            Self::Console => writeln!(io::stderr().lock(), "{message}"),
            #[cfg(unix)]
            Self::Datagram(socket) => socket.send(packet.as_bytes()).map(|_| ()),
            #[cfg(unix)]
            Self::Stream(socket) => socket.write_all(packet.as_bytes()),
        }
    }
}

use ::log::{Level, Log, Metadata, Record};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

pub struct Logger {
    handlers: Mutex<Vec<(Handler, bool)>>,
    debug: AtomicBool,
}
impl Logger {
    pub fn new(handler: Handler, testing: bool) -> Self {
        let logger = Self {
            handlers: Mutex::new(vec![(handler, true)]),
            debug: AtomicBool::new(testing),
        };
        if testing {
            logger.log_to_console();
        }
        logger
    }
    pub fn log_to_console(&self) {
        self.handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((Handler::Console, false));
    }
    pub fn set_log_level_debug(&self) {
        self.debug.store(true, Ordering::Relaxed);
    }
    /// Accept the validated config's `knobs` mapping. Non-mappings are ignored.
    pub fn apply_knobs(&self, knobs: &serde_yaml::Value) {
        if !knobs.is_mapping() {
            return;
        }
        if knobs["log_to_console"].as_bool() == Some(true) {
            self.log_to_console();
        }
        if knobs["log_level_debug"].as_bool() == Some(true) {
            self.set_log_level_debug();
        }
    }
}
impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let target = metadata.target();
        (target == crate::consts::APP_NAME || target.starts_with("system_report::"))
            && metadata.level()
                <= if self.debug.load(Ordering::Relaxed) {
                    Level::Debug
                } else {
                    Level::Info
                }
    }
    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        for (handler, include_app) in self
            .handlers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
        {
            let message = format_record(record, *include_app);
            if handler.emit(record.level(), &message).is_err()
                && !matches!(handler, Handler::Console)
            {
                // A lost syslog socket must not silently swallow future records.
                *handler = Handler::Console;
                let _ = handler.emit(record.level(), &message);
            }
        }
    }
    fn flush(&self) {
        let _ = io::stderr().lock().flush();
    }
}
static LOGGER: OnceLock<Logger> = OnceLock::new();
/// Register once at startup; subsequent registration returns SetLoggerError.
pub fn init_logger(testing: bool) -> Result<&'static Logger, ::log::SetLoggerError> {
    let logger = LOGGER.get_or_init(|| {
        let journal_stream = std::env::var("JOURNAL_STREAM").ok();
        let paths: Vec<_> = LOG_SOCKETS.iter().map(Path::new).collect();
        Logger::new(build_handler(journal_stream.as_deref(), &paths), testing)
    });
    ::log::set_logger(logger)?;
    // Keep the facade ceiling at DEBUG so runtime knobs can enable it later.
    ::log::set_max_level(::log::LevelFilter::Debug);
    Ok(logger)
}
pub fn get_logger() -> Option<&'static Logger> {
    LOGGER.get()
}
fn format_record(record: &Record<'_>, include_app: bool) -> String {
    let timestamp = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S,%3f")
        .to_string();
    let app = if include_app {
        format!(" [{}]", crate::consts::APP_NAME)
    } else {
        String::new()
    };
    let module = record
        .module_path()
        .unwrap_or("unknown")
        .rsplit("::")
        .next()
        .unwrap_or("unknown");
    let level = match record.level() {
        Level::Warn => "WARNING",
        Level::Error => "ERROR",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    };
    format!(
        "{timestamp}{app} {module:>12}:{} {level:<8} {}",
        record.line().unwrap_or(0),
        record.args()
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn logger_filters_levels_formats_records_and_applies_knobs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let socket = UnixDatagram::bind(&path).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let logger = Logger::new(build_handler(None, &[&path]), false);
        let debug = Metadata::builder()
            .target("system_report")
            .level(Level::Debug)
            .build();
        assert!(!logger.enabled(&debug));
        logger.apply_knobs(&serde_yaml::Value::Null);
        logger.apply_knobs(
            &serde_yaml::from_str("log_level_debug: false\nlog_to_console: false").unwrap(),
        );
        assert!(!logger.enabled(&debug));
        assert_eq!(logger.handlers.lock().unwrap().len(), 1);
        logger.apply_knobs(
            &serde_yaml::from_str("log_level_debug: true\nlog_to_console: true").unwrap(),
        );
        assert!(logger.enabled(&debug));
        assert_eq!(logger.handlers.lock().unwrap().len(), 2);
        assert!(!logger.enabled(&Metadata::builder().level(Level::Trace).build()));
        assert!(
            !logger.enabled(
                &Metadata::builder()
                    .level(Level::Info)
                    .target("other_crate")
                    .build()
            )
        );
        logger.log(
            &Record::builder()
                .target("system_report::mqttclient")
                .module_path(Some("system_report::mqttclient"))
                .line(Some(42))
                .level(Level::Warn)
                .args(format_args!("hello"))
                .build(),
        );
        let mut buf = [0; 512];
        let n = socket.recv(&mut buf).unwrap();
        let message = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(message.starts_with("<28>"));
        assert!(
            message.ends_with(" [system_report]   mqttclient:42 WARNING  hello\0"),
            "{message:?}"
        );
        let record = Record::builder()
            .level(Level::Info)
            .module_path(Some("m"))
            .line(Some(7))
            .args(format_args!("hi"))
            .build();
        assert!(format_record(&record, false).ends_with("            m:7 INFO     hi"));
        assert!(!format_record(&record, false).contains("[system_report]"));
        logger.flush();
        let testing = Logger::new(Handler::Console, true);
        assert!(testing.enabled(&debug));
        assert_eq!(testing.handlers.lock().unwrap().len(), 2);
    }

    #[test]
    fn logger_filters_before_emission_and_falls_back_if_socket_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let socket = UnixDatagram::bind(&path).unwrap();
        let logger = Logger::new(build_handler(None, &[&path]), false);
        socket.set_nonblocking(true).unwrap();
        logger.log(
            &Record::builder()
                .target("system_report")
                .level(Level::Debug)
                .args(format_args!("filtered"))
                .build(),
        );
        let mut buf = [0; 512];
        assert_eq!(
            socket.recv(&mut buf).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(socket);
        logger.log(
            &Record::builder()
                .target("system_report")
                .level(Level::Info)
                .args(format_args!("fallback"))
                .build(),
        );
        assert!(matches!(
            logger.handlers.lock().unwrap()[0].0,
            Handler::Console
        ));
    }

    #[test]
    fn global_logger_initializes_and_registers_with_log_facade() {
        let logger = init_logger(true).unwrap();
        assert!(std::ptr::eq(logger, get_logger().unwrap()));
        assert!(::log::log_enabled!(target: "system_report", Level::Debug));
        ::log::info!(target: "system_report", "logger facade smoke test");
        assert!(init_logger(false).is_err());
    }

    #[test]
    fn journal_capture_wins_and_default_sockets_do_not_bypass_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("syslog");
        let _socket = UnixDatagram::bind(&path).unwrap();
        assert!(matches!(
            build_handler(Some("9:1328863"), &[&path]),
            Handler::Console
        ));
        assert!(LOG_SOCKETS.contains(&"/dev/log"));
        assert!(!LOG_SOCKETS.contains(&"/run/systemd/journal/syslog"));
    }

    #[test]
    fn only_actual_usable_sockets_are_selected() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let regular = dir.path().join("regular");
        std::fs::write(&regular, "").unwrap();
        assert_eq!(log_handler_address(&[&missing, &regular]), None);
        assert!(matches!(
            build_handler(None, &[&missing, &regular]),
            Handler::Console
        ));
        let path = dir.path().join("socket");
        let socket = UnixDatagram::bind(&path).unwrap();
        assert_eq!(
            log_handler_address(&[&missing, &regular, &path]),
            Some(path.clone())
        );
        assert!(matches!(
            build_handler(Some(""), &[&path]),
            Handler::Datagram(_)
        ));
        drop(socket); // stale socket inode: connect fails and must fall back
        assert!(matches!(build_handler(None, &[&path]), Handler::Console));
    }

    #[test]
    fn syslog_sends_daemon_facility_priority_and_nul_terminated_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("socket");
        let socket = UnixDatagram::bind(&path).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut handler = build_handler(None, &[&path]);
        for (level, priority) in [
            (::log::Level::Error, 27),
            (::log::Level::Warn, 28),
            (::log::Level::Info, 30),
            (::log::Level::Debug, 31),
            (::log::Level::Trace, 31),
        ] {
            handler.emit(level, "hello").unwrap();
            let mut buf = [0; 256];
            let n = socket.recv(&mut buf).unwrap();
            assert_eq!(&buf[..n], format!("<{priority}>hello\0").as_bytes());
        }
        drop(socket);
        assert!(handler.emit(::log::Level::Info, "gone").is_err());
    }

    #[test]
    fn stream_syslog_is_supported_too() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let mut handler = build_handler(None, &[&path]);
        assert!(matches!(handler, Handler::Stream(_)));
        handler.emit(::log::Level::Info, "hello").unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0; 10];
        socket.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"<30>hello\0");
    }
}
