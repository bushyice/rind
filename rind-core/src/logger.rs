use crate::config::CONFIG;
use crate::services::Service;
use anyhow::Result;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
  collections::HashMap,
  fs::{self, File, OpenOptions},
  io::{self, BufRead, BufReader, Write},
  path::Path,
  process::Child,
  sync::{
    Arc,
    mpsc::{self, SendError, Sender},
  },
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

pub static LOGGER: Lazy<Arc<Sender<LogEntry>>> = Lazy::new(|| start_logger());

#[derive(Serialize, Deserialize)]
pub enum LogLevel {
  Info,
  Error,
  Warn,
  Trace,
  Debug,
  Fatal,
}

#[derive(Serialize, Deserialize)]
pub struct LogEntry {
  pub timestamp: u64,
  pub service: String,
  pub pid: u32,
  pub level: LogLevel,
  pub message: String,
  pub fields: Option<HashMap<String, String>>,
}

pub fn start_logger() -> Arc<Sender<LogEntry>> {
  let log_path = {
    let conf = CONFIG.read().unwrap();
    conf.logger.log_path.clone()
  };

  fs::create_dir_all(log_path.as_str()).expect("failed to create log dir");

  let (tx, rx) = mpsc::channel::<LogEntry>();
  let tx = Arc::new(tx);

  thread::spawn(move || {
    let mut file = open_log_file(log_path.as_str()).expect("log open failed");

    while let Ok(event) = rx.recv() {
      write_entry(&mut file, &event).ok();
    }
  });

  tx
}

fn open_log_file(log_dir: &str) -> Result<File> {
  let log_path = Path::new(log_dir).join("current.rlog");
  let new_file = !log_path.exists();

  let mut file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(log_path)?;

  if new_file {
    file.write_all(b"RINDLOG1")?;
  }

  Ok(file)
}

fn print_log(entry: &LogEntry) {
  let ts = UNIX_EPOCH + Duration::from_secs(entry.timestamp);
  let datetime = ts
    .duration_since(UNIX_EPOCH)
    .unwrap_or(Duration::from_secs(0))
    .as_secs();

  let tm = {
    let days = datetime / 86400;
    let secs_in_day = datetime % 86400;
    let hour = secs_in_day / 3600;
    let min = (secs_in_day % 3600) / 60;
    let sec = secs_in_day % 60;
    (days, hour, min, sec)
  };

  let (days, hour, min, sec) = tm;
  let ts_str = format!("Day {} {:02}:{:02}:{:02}", days, hour, min, sec);

  let level = match entry.level {
    LogLevel::Info => "INFO",
    LogLevel::Error => "ERROR",
    LogLevel::Warn => "WARN",
    LogLevel::Trace => "TRACE",
    LogLevel::Debug => "DEBUG",
    LogLevel::Fatal => "FATAL",
  };

  println!(
    "[{}] [{}:{}] {}: {}",
    ts_str, entry.service, entry.pid, level, entry.message
  );

  io::stdout().flush().ok();
}

fn write_entry(file: &mut File, entry: &LogEntry) -> Result<()> {
  let json = serde_json::to_vec(entry)?;
  let len = json.len() as u64;

  file.write_all(&len.to_be_bytes())?;
  file.write_all(&json)?;
  file.flush()?;

  print_log(entry);

  Ok(())
}

pub fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_secs()
}

pub fn log_child(child: &mut Child, service: &Service, logger: Arc<Sender<LogEntry>>) {
  let pid = child.id();

  if let Some(stdout) = child.stdout.take() {
    let service_name = service.name.clone();
    let logger_clone = logger.clone();

    thread::spawn(move || {
      let reader = BufReader::new(stdout);
      for line in reader.lines().flatten() {
        logger_clone
          .send(LogEntry {
            timestamp: now(),
            service: service_name.clone(),
            pid,
            level: LogLevel::Info,
            message: line,
            fields: None,
          })
          .ok();
      }
    });
  }

  if let Some(stderr) = child.stderr.take() {
    let service_name = service.name.clone();
    let logger_clone = logger.clone();

    thread::spawn(move || {
      let reader = BufReader::new(stderr);
      for line in reader.lines().flatten() {
        logger_clone
          .send(LogEntry {
            timestamp: now(),
            service: service_name.clone(),
            pid,
            level: LogLevel::Error,
            message: line,
            fields: None,
          })
          .ok();
      }
    });
  }
}

pub fn log_event(log: LogEntry) -> Result<(), SendError<LogEntry>> {
  LOGGER.send(log)
}

#[macro_export]
macro_rules! logentry {
  ($type:ident, $service:expr, $pid:expr, $msg:expr) => {{
    $crate::logger::log_event($crate::logger::LogEntry {
      timestamp: $crate::logger::now(),
      service: $service.to_string(),
      pid: $pid,
      level: $crate::logger::LogLevel::$type,
      message: $msg.to_string(),
      fields: None,
    })
    .ok();
  }};
}

#[macro_export]
macro_rules! loginfo_as {
  ($msg:expr) => {
    $crate::logentry!(Info, "rind", std::process::id(), $msg)
  };
  ($service:expr, $msg:expr) => {
    $crate::logentry!(Info, $service, std::process::id(), $msg)
  };
  ($service:expr, $pid:expr, $msg:expr) => {
    $crate::logentry!(Info, $service, $pid, $msg)
  };
}

#[macro_export]
macro_rules! loginfo {
  ($($arg:tt)*) => {
    $crate::logentry!(
      Info,
      "rind",
      std::process::id(),
      &format!($($arg)*)
    )
  };
}

#[macro_export]
macro_rules! logerr_as {
  ($msg:expr) => {
    $crate::logentry!(Error, "rind", std::process::id(), $msg)
  };
  ($service:expr, $msg:expr) => {
    $crate::logentry!(Error, $service, std::process::id(), $msg)
  };
  ($service:expr, $pid:expr, $msg:expr) => {
    $crate::logentry!(Error, $service, $pid, $msg)
  };
}

#[macro_export]
macro_rules! logerr {
  ($($arg:tt)*) => {
    $crate::logentry!(
      Info,
      "rind",
      std::process::id(),
      &format!($($arg)*)
    )
  };
}

#[macro_export]
macro_rules! logwarn_as {
  ($msg:expr) => {
    $crate::logentry!(Warn, "rind", std::process::id(), $msg)
  };
  ($service:expr, $msg:expr) => {
    $crate::logentry!(Warn, $service, std::process::id(), $msg)
  };
  ($service:expr, $pid:expr, $msg:expr) => {
    $crate::logentry!(Warn, $service, $pid, $msg)
  };
}

#[macro_export]
macro_rules! logwarn {
  ($($arg:tt)*) => {
    $crate::logentry!(
      Warn,
      "rind",
      std::process::id(),
      &format!($($arg)*)
    )
  };
}

#[macro_export]
macro_rules! logtrc_as {
  ($msg:expr) => {
    $crate::logentry!(Trace, "rind", std::process::id(), $msg)
  };
  ($service:expr, $msg:expr) => {
    $crate::logentry!(Trace, $service, std::process::id(), $msg)
  };
  ($service:expr, $pid:expr, $msg:expr) => {
    $crate::logentry!(Trace, $service, $pid, $msg)
  };
}

#[macro_export]
macro_rules! logtrc {
  ($($arg:tt)*) => {
    $crate::logentry!(
      Trace,
      "rind",
      std::process::id(),
      &format!($($arg)*)
    )
  };
}
