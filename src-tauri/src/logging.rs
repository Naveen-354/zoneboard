//! Structured logging with daily file rotation and zero-leak secret redaction.
//!
//! # Architecture & Guarantees
//! - Emits structured logs using [`tracing`] to both `std::io::stderr` and a daily file in the
//!   application data directory (`<data_dir>/zoneboard-YYYY-MM-DD.log`).
//! - A single open file handle is held behind a mutex and is reopened only when the date rolls over,
//!   preventing per-event file open overhead.
//! - **Redaction Layer:** Automatically masks fields whose names indicate sensitive material
//!   (`api_key`, `secret`, `token`, `password`, `authorization`, `signature`) as `[REDACTED]`.
//! - **Secrecy Integration:** Values wrapped in `secrecy::SecretBox`, `secrecy::SecretString`, or
//!   `secrecy::Secret` naturally format as `[REDACTED]` under their `Debug` implementation.
//!
//! # Critical Boundary & Repository Convention
//! > [!IMPORTANT]
//! > **Field-name redaction only inspects structured fields.** A secret formatted directly into the
//! > message text (e.g. `tracing::info!("api_key is {}", key)`) **is NOT masked**.
//! >
//! > **Repository Rule:** Secrets must NEVER be interpolated into log message format strings.
//! > Always place sensitive values into structured fields (e.g. `tracing::info!(api_key = %key, ...)`),
//! > or wrap them in [`secrecy::SecretString`] / [`secrecy::SecretBox`].

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};

use chrono::Utc;
use tracing::{field::Field, field::Visit, Event, Subscriber};
use tracing_subscriber::{
    fmt::{format::Writer, FmtContext, FormatEvent, FormatFields},
    registry::LookupSpan,
    EnvFilter,
};

/// Returns true if a field name indicates secret or sensitive credential data.
pub fn is_secret_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    const PATTERNS: &[&str] = &[
        "api_key",
        "secret",
        "token",
        "password",
        "authorization",
        "signature",
    ];
    PATTERNS.iter().any(|&p| lower.contains(p))
}

/// Visitor that extracts log fields while replacing sensitive fields with `[REDACTED]`.
#[derive(Default)]
pub struct RedactingVisitor {
    pub message: Option<String>,
    pub fields: Vec<(String, String)>,
}

impl Visit for RedactingVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let name = field.name();
        if name == "message" {
            // Note: in tracing, the formatted message arrives here.
            // By design, interpolated message text is passed through as-is.
            self.message = Some(format!("{value:?}"));
        } else if is_secret_field(name) {
            self.fields.push((name.to_string(), "[REDACTED]".to_string()));
        } else {
            self.fields.push((name.to_string(), format!("{value:?}")));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        let name = field.name();
        if name == "message" {
            self.message = Some(value.to_string());
        } else if is_secret_field(name) {
            self.fields.push((name.to_string(), "[REDACTED]".to_string()));
        } else {
            self.fields.push((name.to_string(), value.to_string()));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        let name = field.name();
        if is_secret_field(name) {
            self.fields.push((name.to_string(), "[REDACTED]".to_string()));
        } else {
            self.fields.push((name.to_string(), value.to_string()));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let name = field.name();
        if is_secret_field(name) {
            self.fields.push((name.to_string(), "[REDACTED]".to_string()));
        } else {
            self.fields.push((name.to_string(), value.to_string()));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        let name = field.name();
        if is_secret_field(name) {
            self.fields.push((name.to_string(), "[REDACTED]".to_string()));
        } else {
            self.fields.push((name.to_string(), value.to_string()));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let name = field.name();
        if is_secret_field(name) {
            self.fields.push((name.to_string(), "[REDACTED]".to_string()));
        } else {
            self.fields.push((name.to_string(), value.to_string()));
        }
    }
}

/// Custom tracing event formatter that redacts sensitive fields and writes consistent ISO-8601 logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct RedactingFormatter;

impl<S, N> FormatEvent<S, N> for RedactingFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        write!(writer, "{now} {:5} {}: ", meta.level().as_str(), meta.target())?;

        let mut visitor = RedactingVisitor::default();
        event.record(&mut visitor);

        if let Some(msg) = visitor.message {
            write!(writer, "{msg}")?;
        }

        for (k, v) in visitor.fields {
            write!(writer, " {k}={v}")?;
        }

        writeln!(writer)
    }
}

/// A rolling daily file writer keeping a single open file handle behind a mutex,
/// reopening only when the current date changes.
pub struct DailyFileWriter {
    dir: PathBuf,
    state: Mutex<Option<(String, File)>>,
}

impl DailyFileWriter {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            state: Mutex::new(None),
        }
    }

    pub fn write_buf(&self, buf: &[u8]) -> io::Result<usize> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let mut state = self.state.lock().unwrap();
        let needs_reopen = match &*state {
            Some((date, _)) => date != &today,
            None => true,
        };

        if needs_reopen {
            std::fs::create_dir_all(&self.dir)?;
            let path = self.dir.join(format!("zoneboard-{today}.log"));
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            *state = Some((today, file));
        }

        if let Some((_, ref mut file)) = *state {
            file.write_all(buf)?;
            file.flush()?;
        }
        Ok(buf.len())
    }
}

/// Writer sending log lines simultaneously to stderr and the daily log file.
#[derive(Clone)]
pub struct DualLogWriter {
    file: Arc<DailyFileWriter>,
}

impl Write for DualLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        let _ = io::stderr().flush();
        self.file.write_buf(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for DualLogWriter {
    type Writer = DualLogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

static INIT_ONCE: Once = Once::new();

/// Initializes structured logging to both a daily rolling log file in `data_dir` and stderr.
///
/// Idempotent: safe to invoke multiple times (subsequent calls are no-ops).
pub fn init(data_dir: &Path) {
    INIT_ONCE.call_once(|| {
        let file = Arc::new(DailyFileWriter::new(data_dir.to_path_buf()));
        let writer = DualLogWriter { file };
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info"));
        let subscriber = tracing_subscriber::fmt()
            .event_format(RedactingFormatter)
            .with_writer(writer)
            .with_env_filter(env_filter)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

#[cfg(test)]
#[derive(Clone)]
struct TestBuffer(Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl io::Write for TestBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap().flush()
    }
}

#[cfg(test)]
impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for TestBuffer {
    type Writer = TestBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::{SecretBox, SecretString};
    use std::sync::{Arc, Mutex};
    use tracing::info;

    #[test]
    fn secret_fields_are_redacted_in_structured_logs() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .event_format(RedactingFormatter)
            .with_writer(TestBuffer(buffer.clone()))
            .with_max_level(tracing::Level::TRACE)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let plain_token = "my-super-secret-token-12345";
            let plain_key = "api-key-99999";
            let plain_pw = "pass123";
            let safe_val = "safe-username";

            info!(
                token = plain_token,
                api_key = plain_key,
                password = plain_pw,
                user = safe_val,
                port = 5000,
                "login attempt"
            );
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();

        // Safe values and message are preserved
        assert!(output.contains("login attempt"), "missing message: {output}");
        assert!(output.contains("user=safe-username"), "missing safe field: {output}");
        assert!(output.contains("port=5000"), "missing port: {output}");

        // Sensitive field names are masked
        assert!(output.contains("token=[REDACTED]"), "token not redacted: {output}");
        assert!(output.contains("api_key=[REDACTED]"), "api_key not redacted: {output}");
        assert!(output.contains("password=[REDACTED]"), "password not redacted: {output}");

        // Plaintext values must NOT appear anywhere in the output
        assert!(!output.contains("my-super-secret-token-12345"), "leaked token: {output}");
        assert!(!output.contains("api-key-99999"), "leaked key: {output}");
        assert!(!output.contains("pass123"), "leaked password: {output}");
    }

    #[test]
    fn secrecy_wrapper_values_are_masked() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .event_format(RedactingFormatter)
            .with_writer(TestBuffer(buffer.clone()))
            .with_max_level(tracing::Level::TRACE)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let secret_str: SecretString = SecretString::from("hidden-credential");
            let secret_box: SecretBox<String> = SecretBox::new(Box::new("hidden-box".to_string()));

            // Even if field names are innocuously named, secrecy types mask their content in Debug
            info!(
                custom_field = ?secret_str,
                boxed_field = ?secret_box,
                "secrecy verification"
            );
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(output.contains("[REDACTED]"), "output missing redaction: {output}");
        assert!(!output.contains("hidden-credential"), "leaked SecretString: {output}");
        assert!(!output.contains("hidden-box"), "leaked SecretBox: {output}");
    }

    #[test]
    fn interpolated_secrets_in_message_are_not_masked_documenting_real_limit() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .event_format(RedactingFormatter)
            .with_writer(TestBuffer(buffer.clone()))
            .with_max_level(tracing::Level::TRACE)
            .finish();

        let raw_secret = "raw-secret-in-format-string";
        tracing::subscriber::with_default(subscriber, || {
            // Documented real limit: interpolating into message text does not mask.
            // Repo standing convention is that secrets never go into message text.
            info!("authentication key: {}", raw_secret);
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(
            output.contains("authentication key: raw-secret-in-format-string"),
            "format text must pass through demonstrating documented boundary: {output}"
        );
    }

    #[test]
    fn daily_file_writer_creates_log_file_and_appends() {
        let temp_dir = std::env::temp_dir().join(format!("zb_log_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        let writer = DailyFileWriter::new(temp_dir.clone());
        writer.write_buf(b"line 1\n").unwrap();
        writer.write_buf(b"line 2\n").unwrap();

        let today = Utc::now().format("%Y-%m-%d").to_string();
        let expected_file = temp_dir.join(format!("zoneboard-{today}.log"));
        assert!(expected_file.exists(), "log file was not created");

        let content = std::fs::read_to_string(&expected_file).unwrap();
        assert_eq!(content, "line 1\nline 2\n");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
