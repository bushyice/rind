/*
 * TODO: Userspace Update
 * - spaces (user/system), add a user field to logging
 */

use std::collections::HashMap;
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LogLevel {
  Trace,
  Debug,
  Info,
  Warn,
  Error,
  Fatal,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
  pub timestamp: u64,
  pub level: LogLevel,
  pub target: String,
  pub message: String,
  pub fields: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct LogConfig {
  pub dir: PathBuf,
  pub flush_interval: Duration,
  pub segment_max_bytes: u64,
}

impl Default for LogConfig {
  fn default() -> Self {
    Self {
      dir: PathBuf::from("/var/log/rind"),
      flush_interval: Duration::from_millis(250),
      segment_max_bytes: 16 * 1024 * 1024,
    }
  }
}

#[derive(Clone)]
pub struct LogHandle {
  tx: Sender<LogEntry>,
}

impl LogHandle {
  pub fn log(
    &self,
    level: LogLevel,
    target: impl Into<String>,
    message: impl Into<String>,
    fields: HashMap<String, String>,
  ) {
    let _ = self.tx.send(LogEntry {
      timestamp: now_unix_sec(),
      level,
      target: target.into(),
      message: message.into(),
      fields,
    });
  }
}

pub fn start_logger(config: LogConfig) -> LogHandle {
  let (tx, rx) = mpsc::channel::<LogEntry>();
  thread::spawn(move || logger_loop(config, rx));
  LogHandle { tx }
}

fn timestamp_fmt(timestamp: u64) -> String {
  let s = unsafe {
    #[allow(deprecated)]
    let t = timestamp as libc::time_t;
    let mut tm: libc::tm = std::mem::zeroed();

    libc::localtime_r(&t, &mut tm);

    let mut buf = [0u8; 64];
    let fmt = std::ffi::CString::new("%d/%m/%y %H:%M:%S").unwrap();

    libc::strftime(
      buf.as_mut_ptr() as *mut libc::c_char,
      buf.len(),
      fmt.as_ptr(),
      &tm,
    );

    std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char).to_string_lossy()
  };

  s.to_string()
}

fn logger_loop(config: LogConfig, rx: Receiver<LogEntry>) {
  if let Err(err) = create_dir_all(config.dir.as_path()) {
    eprintln!(
      "logger: failed to create log dir '{}': {err}",
      config.dir.display()
    );
  }

  let mut segment_id = 1u64;
  let mut written = 0u64;
  let (mut writer, mut current_path) = open_segment(config.dir.as_path(), segment_id);

  loop {
    let Ok(entry) = rx.recv_timeout(config.flush_interval) else {
      let _ = writer.flush();
      continue;
    };

    println!(
      "[{:?} {}] {{{}}}: {} ({:?})",
      entry.level,
      timestamp_fmt(entry.timestamp),
      entry.target,
      entry.message,
      entry.fields
    );

    match encode_record(&entry) {
      Ok(bytes) => {
        if let Err(err) = writer.write_all(&bytes) {
          eprintln!(
            "logger: failed to write to '{}': {err}",
            current_path.display()
          );
          continue;
        }

        written += bytes.len() as u64;

        if let Err(err) = writer.flush() {
          eprintln!(
            "logger: failed to flush '{}': {err}",
            current_path.display()
          );
        }
      }
      Err(err) => {
        eprintln!("logger: failed to encode entry: {err}");
      }
    }

    if written >= config.segment_max_bytes {
      let _ = writer.flush();
      let _ = writer.get_ref().sync_data();
      segment_id += 1;
      (writer, current_path) = open_segment(config.dir.as_path(), segment_id);
      written = 0;
    }
  }
}

fn open_segment(dir: &Path, id: u64) -> (BufWriter<File>, PathBuf) {
  const FALLBACK_LOG_PATH: &str = "/var/log/rind-fallback.rlog";

  let path = dir.join(format!("{id:08}.rlog"));

  match OpenOptions::new().create(true).append(true).open(&path) {
    Ok(file) => (BufWriter::new(file), path),
    Err(err) => {
      eprintln!(
        "logger: failed to open segment in '{}': {err}; using fallback '{}'",
        dir.display(),
        FALLBACK_LOG_PATH
      );
      let fallback = PathBuf::from(FALLBACK_LOG_PATH);
      let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&fallback)
        .unwrap_or_else(|err| panic!("failed to open fallback log file: {err}")); // change (if not opened, don't crash)
      (BufWriter::new(file), fallback)
    }
  }
}

const MAGIC: u32 = 0x524C4F47; // "RLOG"

fn encode_record(entry: &LogEntry) -> Result<Vec<u8>, String> {
  let cfg = bincode_next::config::standard();
  let payload = bincode_next::serde::encode_to_vec(entry, cfg).map_err(|e| e.to_string())?;
  let payload_len = payload.len() as u32;
  let crc = crc32fast::hash(&payload);

  let total_len = 4 + payload_len + 4;
  let mut out = Vec::with_capacity(4 + 4 + total_len as usize);
  out.extend_from_slice(&MAGIC.to_be_bytes());
  out.extend_from_slice(&total_len.to_be_bytes());
  out.extend_from_slice(&payload_len.to_be_bytes());
  out.extend_from_slice(&payload);
  out.extend_from_slice(&crc.to_be_bytes());
  Ok(out)
}

fn now_unix_sec() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
