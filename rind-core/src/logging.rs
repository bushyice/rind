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
      dir: PathBuf::from("/tmp/logs"),
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

fn logger_loop(config: LogConfig, rx: Receiver<LogEntry>) {
  let _ = create_dir_all(config.dir.as_path());

  let mut segment_id = 1u64;
  let mut written = 0u64;
  let mut writer = open_segment(config.dir.as_path(), segment_id);

  loop {
    let Ok(entry) = rx.recv_timeout(config.flush_interval) else {
      let _ = writer.flush();
      continue;
    };

    let line = serde_json::to_string(&entry).unwrap_or_else(|_| "{}".to_string());
    let bytes = line.as_bytes();
    if writer.write_all(bytes).is_ok() && writer.write_all(b"\n").is_ok() {
      written += (bytes.len() + 1) as u64;
    }

    if written >= config.segment_max_bytes {
      let _ = writer.flush();
      segment_id += 1;
      writer = open_segment(config.dir.as_path(), segment_id);
      written = 0;
    }
  }
}

fn open_segment(dir: &Path, id: u64) -> BufWriter<File> {
  let path = dir.join(format!("{id:08}.jsonl"));
  let file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(path)
    .unwrap_or_else(|_| {
      OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/rind-fallback.jsonl")
        .unwrap_or_else(|err| panic!("failed to open fallback log file: {err}"))
    });
  BufWriter::new(file)
}

fn now_unix_sec() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_secs()
}
