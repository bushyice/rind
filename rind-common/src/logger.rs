use crate::config::CONFIG;
use anyhow::Result;
use bincode_next::{Decode, Encode, config};
use once_cell::sync::Lazy;
use std::{
  collections::HashMap,
  fs::{self, File, OpenOptions},
  io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
  path::{Path, PathBuf},
  process::Child,
  sync::{
    Arc,
    mpsc::{self, SendError, Sender},
  },
  thread,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// says RLOG
const MAGIC: u32 = 0x524C4F47;

pub static LOGGER: Lazy<Arc<Sender<LogEntry>>> = Lazy::new(|| start_logger());

#[derive(Encode, Decode, Clone, Copy)]
pub enum LogLevel {
  Info,
  Error,
  Warn,
  Trace,
  Debug,
  Fatal,
}

#[derive(Encode, Decode)]
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
    conf.logger.log_path.to_string()
  };

  fs::create_dir_all(log_path.as_str()).expect("failed to create log dir");

  let (tx, rx) = mpsc::channel::<LogEntry>();
  let tx = Arc::new(tx);

  thread::spawn(move || {
    logger_thread(rx, log_path);
  });

  tx
}

fn logger_thread(rx: mpsc::Receiver<LogEntry>, dir: impl Into<PathBuf>) {
  let dir = dir.into();
  let conf = CONFIG.read().unwrap();
  let conf = &conf.logger;

  let mut segment_id = next_segment_id(&dir);
  let mut writer = open_segment(&dir, segment_id);

  let mut current_size = 0u64;
  let mut last_flush = Instant::now();
  let mut last_sync = Instant::now();

  let mut batch = Vec::with_capacity(conf.batch_size);

  loop {
    batch.clear();

    for _ in 0..conf.batch_size {
      match rx.recv_timeout(Duration::from_millis(conf.flush_interval)) {
        Ok(entry) => batch.push(entry),
        Err(mpsc::RecvTimeoutError::Timeout) => break,
        Err(_) => return,
      }
    }

    if batch.is_empty() {
      continue;
    }

    for entry in &batch {
      if let Ok(written) = write_record(&mut writer, entry) {
        current_size += written as u64;
      }
    }

    if current_size >= conf.max_segment_size {
      writer.flush().ok();
      writer.get_ref().sync_data().ok();

      segment_id += 1;
      writer = open_segment(&dir, segment_id);
      current_size = 0;
    }

    if last_flush.elapsed().as_millis() > conf.flush_interval as u128 {
      writer.flush().ok();
      last_flush = Instant::now();
    }

    if last_sync.elapsed().as_secs() >= conf.fsync_interval {
      writer.get_ref().sync_data().ok();
      last_sync = Instant::now();
    }
  }
}

fn next_segment_id(dir: &Path) -> u64 {
  fs::read_dir(dir)
    .unwrap()
    .filter_map(|e| e.ok())
    .filter_map(|e| {
      e.path()
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u64>().ok())
    })
    .max()
    .unwrap_or(0)
    + 1
}

fn open_segment(dir: &Path, id: u64) -> BufWriter<File> {
  let path = dir.join(format!("{:08}.rlog", id));

  let file = OpenOptions::new()
    .create(true)
    .append(true)
    .open(path)
    .expect("segment open failed");

  BufWriter::with_capacity(64 * 1024, file)
}

fn write_record(writer: &mut BufWriter<File>, entry: &LogEntry) -> anyhow::Result<usize> {
  let config = config::standard();

  print_log(entry);

  let payload = bincode_next::encode_to_vec(entry, config)?;
  let payload_len = payload.len() as u32;

  let timestamp = SystemTime::now()
    .duration_since(SystemTime::UNIX_EPOCH)?
    .as_secs();

  let level = entry.level as u8;
  let service_hash = fxhash::hash64(&entry.service);

  let total_len = 4 + // total_len field
        8 + // timestamp
        1 + // level
        8 + // service_hash
        4 + // payload_len
        payload_len +
        4; // crc

  let mut hasher = crc32fast::Hasher::new();
  hasher.update(&total_len.to_be_bytes());
  hasher.update(&timestamp.to_be_bytes());
  hasher.update(&[level]);
  hasher.update(&service_hash.to_be_bytes());
  hasher.update(&payload_len.to_be_bytes());
  hasher.update(&payload);

  let crc = hasher.finalize();

  writer.write_all(&MAGIC.to_be_bytes())?;
  writer.write_all(&(total_len as u32).to_be_bytes())?;
  writer.write_all(&timestamp.to_be_bytes())?;
  writer.write_all(&[level])?;
  writer.write_all(&service_hash.to_be_bytes())?;
  writer.write_all(&payload_len.to_be_bytes())?;
  writer.write_all(&payload)?;
  writer.write_all(&crc.to_be_bytes())?;

  Ok((4 + total_len) as usize)
}

pub fn query_segment(
  path: &Path,
  service_filter: Option<u64>,
  min_level: Option<u8>,
  start_ts: Option<u64>,
) -> anyhow::Result<Vec<LogEntry>> {
  let config = config::standard();
  let file = File::open(path)?;
  let mut reader = BufReader::new(file);

  let mut results = Vec::new();

  loop {
    let mut magic_buf = [0u8; 4];
    if reader.read_exact(&mut magic_buf).is_err() {
      break;
    }

    if u32::from_be_bytes(magic_buf) != MAGIC {
      break;
    }

    let mut header_buf = [0u8; 4 + 8 + 1 + 8 + 4];
    reader.read_exact(&mut header_buf)?;

    let mut offset = 0;

    // let total_len = u32::from_be_bytes(header_buf[offset..offset + 4].try_into()?);
    offset += 4;

    let timestamp = u64::from_be_bytes(header_buf[offset..offset + 8].try_into()?);
    offset += 8;

    let level = header_buf[offset];
    offset += 1;

    let service_hash = u64::from_be_bytes(header_buf[offset..offset + 8].try_into()?);
    offset += 8;

    let payload_len = u32::from_be_bytes(header_buf[offset..offset + 4].try_into()?);

    // FILTER WITHOUT DESERIALIZE
    if let Some(min) = min_level {
      if level < min {
        reader.seek(SeekFrom::Current((payload_len + 4) as i64))?;
        continue;
      }
    }

    if let Some(filter_hash) = service_filter {
      if service_hash != filter_hash {
        reader.seek(SeekFrom::Current((payload_len + 4) as i64))?;
        continue;
      }
    }

    if let Some(start) = start_ts {
      if timestamp < start {
        reader.seek(SeekFrom::Current((payload_len + 4) as i64))?;
        continue;
      }
    }

    let mut payload = vec![0u8; payload_len as usize];
    reader.read_exact(&mut payload)?;

    let mut crc_buf = [0u8; 4];
    reader.read_exact(&mut crc_buf)?;
    let stored_crc = u32::from_be_bytes(crc_buf);

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&header_buf);
    hasher.update(&payload);
    let computed = hasher.finalize();

    if computed != stored_crc {
      break;
    }

    let (entry, _) = bincode_next::decode_from_slice(&payload, config)?;
    results.push(entry);
  }

  Ok(results)
}

fn all_segments(dir: &Path) -> Vec<PathBuf> {
  let mut segs: Vec<_> = fs::read_dir(dir)
    .unwrap()
    .filter_map(|e| e.ok())
    .map(|e| e.path())
    .filter(|p| p.extension().map(|ext| ext == "rlog").unwrap_or(false))
    .collect();

  segs.sort(); // ascending by filename (segment id)
  segs
}

pub fn query_logs(
  dir: impl Into<PathBuf>,
  service_name: Option<&str>,
  min_level: Option<LogLevel>,
  start_ts: Option<u64>,
  end_ts: Option<u64>,
) -> anyhow::Result<Vec<LogEntry>> {
  let dir = dir.into();
  let service_hash = service_name.map(|s| fxhash::hash64(s));

  let mut results = Vec::new();

  for seg_path in all_segments(&dir) {
    let mut seg_entries = query_segment(
      &seg_path,
      service_hash,
      min_level.map(|l| l as u8),
      start_ts,
    )?;

    // optional end_ts filter
    if let Some(end) = end_ts {
      seg_entries.retain(|e| e.timestamp <= end);
    }

    results.extend(seg_entries);
  }

  Ok(results)
}

pub fn print_log(entry: &LogEntry) {
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

// fn write_entry(writer: &mut BufWriter<File>, entry: &LogEntry) -> anyhow::Result<()> {
//   let json = serde_json::to_vec(entry)?;
//   let len = json.len() as u64;

//   writer.write_all(&len.to_be_bytes())?;
//   writer.write_all(&json)?;

//   print_log(entry);

//   Ok(())
// }

pub fn now() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_secs()
}

pub fn log_child(child: &mut Child, service: String, logger: Arc<Sender<LogEntry>>) {
  let pid = child.id();

  if let Some(stdout) = child.stdout.take() {
    print!("Took child stdout, {pid}");
    let service_name = service.clone();
    let logger_clone = logger.clone();

    thread::spawn(move || {
      let reader = BufReader::new(stdout);
      for line in reader.lines().flatten() {
        print!("Logging this: {}", line);
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
  } else {
    print!("failed to take child stdout, {pid}");
  }

  if let Some(stderr) = child.stderr.take() {
    let service_name = service.clone();
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
