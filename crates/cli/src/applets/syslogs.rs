use std::{
  collections::HashMap,
  fs::{self, File},
  io::{Read, Seek, SeekFrom, Write},
  path::{Path, PathBuf},
  process::{Child, ChildStdin, Stdio},
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use clap::ValueEnum;
use owo_colors::OwoColorize;
use rind_core::{
  logging::{LogEntry, LogLevel},
  types::Void,
};

use crate::report_error;

const RLOG_MAGIC: u32 = 0x524C4F47;
const FALLBACK_LOG_PATH: &str = "/var/log/rind-fallback.rlog";

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogLevelArg {
  Trace,
  Debug,
  Info,
  Warn,
  Error,
  Fatal,
}

impl From<LogLevelArg> for LogLevel {
  fn from(value: LogLevelArg) -> Self {
    match value {
      LogLevelArg::Trace => LogLevel::Trace,
      LogLevelArg::Debug => LogLevel::Debug,
      LogLevelArg::Info => LogLevel::Info,
      LogLevelArg::Warn => LogLevel::Warn,
      LogLevelArg::Error => LogLevel::Error,
      LogLevelArg::Fatal => LogLevel::Fatal,
    }
  }
}

#[derive(Debug, Clone)]
pub struct LogQuery {
  pub exact: bool,
  pub level: Option<LogLevel>,
  pub target: Option<String>,
  pub message: Option<String>,
  pub since: Option<u64>,
  pub fields: Vec<(String, String)>,
}

impl LogQuery {
  pub fn matches(&self, entry: &LogEntry) -> bool {
    if let Some(min_level) = self.level {
      if if self.exact {
        entry.level == min_level
      } else {
        level_rank(entry.level) < level_rank(min_level)
      } {
        return false;
      }
    }
    if let Some(target) = &self.target {
      if !entry.target.contains(target) {
        return false;
      }
    }
    if let Some(message) = &self.message {
      if !entry.message.contains(message) {
        return false;
      }
    }
    if let Some(since) = self.since {
      if entry.timestamp < since {
        return false;
      }
    }
    self
      .fields
      .iter()
      .all(|(k, v)| entry.fields.get(k).is_some_and(|value| value == v))
  }
}

#[derive(Default)]
pub struct TailCursor {
  pub offset: u64,
  pub carry: Vec<u8>,
}

pub enum OutputSink {
  Stdout,
  Pager { child: Child, stdin: ChildStdin },
}

impl OutputSink {
  pub fn stdout() -> Self {
    Self::Stdout
  }

  pub fn less(follow: bool) -> Result<Self, String> {
    let mut cmd = std::process::Command::new("less");
    cmd.arg("-R");
    if follow {
      cmd.arg("+F");
    }
    let mut child = cmd
      .stdin(Stdio::piped())
      .stdout(Stdio::inherit())
      .stderr(Stdio::inherit())
      .spawn()
      .map_err(|err| format!("failed to start less: {err}"))?;
    let stdin = child
      .stdin
      .take()
      .ok_or_else(|| "failed to open stdin for less".to_string())?;
    Ok(Self::Pager { child, stdin })
  }

  pub fn line(&mut self, line: &str) -> Result<Void, String> {
    match self {
      Self::Stdout => {
        println!("{line}");
        Ok(Void)
      }
      Self::Pager { stdin, .. } => {
        writeln!(stdin, "{line}").map_err(|err| format!("failed to write to less: {err}"))
      }
    }
  }

  pub fn finish(mut self) -> Result<Void, String> {
    match &mut self {
      Self::Stdout => Ok(Void),
      Self::Pager { child, .. } => {
        let _ = child.stdin.take();
        child
          .wait()
          .map(|_| ())
          .map_err(|err| format!("failed while waiting for less: {err}"))
      }
    }
  }
}

pub fn main() {
  use clap::Parser;

  #[derive(Parser)]
  #[command(name = "syslogs")]
  #[command(version = concat!(env!("CARGO_PKG_VERSION"), "-", env!("GIT_HASH"), "-", env!("BUILD_HASH")))]
  struct Cli {
    #[arg(long, default_value = "/var/log/rind")]
    dir: PathBuf,

    #[arg(short = 'l', long)]
    level: Option<LogLevelArg>,

    #[arg(long)]
    target: Option<String>,

    #[arg(long)]
    message: Option<String>,

    #[arg(long)]
    since: Option<u64>,

    #[arg(long)]
    current: bool,

    #[arg(short = 'e', long)]
    exact: bool,

    #[arg(long = "field", value_name = "KEY=VALUE")]
    fields: Vec<String>,

    #[arg(short = 'n', long, default_value_t = 200)]
    limit: usize,

    #[arg(short = 'f', long)]
    tail: bool,

    #[arg(long)]
    less: bool,

    #[arg(long, default_value_t = 500)]
    poll_ms: u64,
  }

  let cli = Cli::parse();

  let since = match resolve_since(cli.since, cli.current) {
    Ok(v) => v,
    Err(err) => {
      report_error("invalid logs query", err);
      return;
    }
  };

  let query = match build_log_query(
    cli.level,
    cli.target,
    cli.message,
    since,
    cli.fields,
    cli.exact,
  ) {
    Ok(query) => query,
    Err(err) => {
      report_error("invalid logs query", err);
      return;
    }
  };

  let mut sink = match if cli.less {
    OutputSink::less(cli.tail)
  } else {
    Ok(OutputSink::stdout())
  } {
    Ok(sink) => sink,
    Err(err) => {
      report_error("logs output setup failed", err);
      return;
    }
  };

  if cli.tail {
    if let Err(err) = tail_logs(cli.dir.as_path(), &query, cli.poll_ms, cli.limit, &mut sink) {
      report_error("logs tail failed", err);
    }
  } else {
    let entries = read_entries_once(cli.dir.as_path(), &query, cli.limit);
    for entry in &entries {
      if let Err(err) = write_log_entry(&mut sink, entry) {
        report_error("logs print failed", err);
        return;
      }
    }
    if entries.is_empty() {
      eprintln!(
        "{} no logs matched in {}",
        "Info".on_cyan().black(),
        cli.dir.display()
      );
    }
    if let Err(err) = sink.finish() {
      report_error("logs output failed", err);
    }
  }
}

fn build_log_query(
  level: Option<LogLevelArg>,
  target: Option<String>,
  message: Option<String>,
  since: Option<u64>,
  raw_fields: Vec<String>,
  exact: bool,
) -> Result<LogQuery, String> {
  let mut fields = Vec::with_capacity(raw_fields.len());
  for field in raw_fields {
    let Some((k, v)) = field.split_once('=') else {
      return Err(format!(
        "invalid --field value '{field}', expected KEY=VALUE"
      ));
    };
    if k.trim().is_empty() {
      return Err(format!(
        "invalid --field value '{field}', key cannot be empty"
      ));
    }
    fields.push((k.trim().to_string(), v.to_string()));
  }
  Ok(LogQuery {
    exact,
    level: level.map(Into::into),
    target,
    message,
    since,
    fields,
  })
}

pub fn get_logs_dir() -> PathBuf {
  std::env::var("RIND_LOG_DIR")
    .map(PathBuf::from)
    .unwrap_or_else(|_| PathBuf::from("/var/log/rind"))
}

fn resolve_since(since: Option<u64>, current: bool) -> Result<Option<u64>, String> {
  if !current {
    return Ok(since);
  }
  let boot_start = current_boot_start_unix()?;
  Ok(Some(since.map(|s| s.max(boot_start)).unwrap_or(boot_start)))
}

pub fn current_boot_start_unix() -> Result<u64, String> {
  if let Ok(stat) = fs::read_to_string("/proc/stat") {
    for line in stat.lines() {
      if let Some(value) = line.strip_prefix("btime ") {
        return value
          .trim()
          .parse::<u64>()
          .map_err(|err| format!("failed to parse /proc/stat btime: {err}"));
      }
    }
  }
  let uptime = fs::read_to_string("/proc/uptime")
    .map_err(|err| format!("failed to read /proc/uptime for --current: {err}"))?;
  let uptime_secs = uptime
    .split_whitespace()
    .next()
    .ok_or_else(|| "invalid /proc/uptime format".to_string())?
    .parse::<f64>()
    .map_err(|err| format!("failed to parse /proc/uptime: {err}"))?;
  let now = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map_err(|err| format!("system clock error: {err}"))?
    .as_secs_f64();
  Ok(if now > uptime_secs {
    (now - uptime_secs) as u64
  } else {
    0
  })
}

pub fn read_entries_once(dir: &Path, query: &LogQuery, limit: usize) -> Vec<LogEntry> {
  let mut matches = Vec::new();
  for segment in list_segments(dir) {
    let Ok(bytes) = fs::read(&segment) else {
      continue;
    };
    let (entries, _) = decode_records(&bytes);
    for entry in entries {
      if query.matches(&entry) {
        matches.push(entry);
      }
    }
  }
  if matches.len() > limit {
    let start = matches.len().saturating_sub(limit);
    return matches.split_off(start);
  }
  matches
}

fn tail_logs(
  dir: &Path,
  query: &LogQuery,
  poll_ms: u64,
  limit: usize,
  sink: &mut OutputSink,
) -> Result<Void, String> {
  eprintln!(
    "{} tailing logs from {} (ctrl+c to stop)",
    "Info".on_cyan().black(),
    dir.display()
  );
  let seed = read_entries_once(dir, query, limit);
  for entry in seed {
    write_log_entry(sink, &entry)?;
  }
  let mut cursors: HashMap<PathBuf, TailCursor> = HashMap::new();
  for segment in list_segments(dir) {
    let offset = fs::metadata(&segment).map(|m| m.len()).unwrap_or_default();
    cursors.insert(
      segment,
      TailCursor {
        offset,
        carry: Vec::new(),
      },
    );
  }
  loop {
    for segment in list_segments(dir) {
      let cursor = cursors.entry(segment.clone()).or_default();
      let entries = read_incremental(segment.as_path(), cursor);
      for entry in entries.into_iter().filter(|entry| query.matches(entry)) {
        write_log_entry(sink, &entry)?;
      }
    }
    thread::sleep(Duration::from_millis(poll_ms));
  }
}

fn read_incremental(path: &Path, cursor: &mut TailCursor) -> Vec<LogEntry> {
  let Ok(mut file) = File::open(path) else {
    return Vec::new();
  };
  let Ok(meta) = file.metadata() else {
    return Vec::new();
  };
  if meta.len() < cursor.offset {
    cursor.offset = 0;
    cursor.carry.clear();
  }
  if file.seek(SeekFrom::Start(cursor.offset)).is_err() {
    return Vec::new();
  }
  let mut appended = Vec::new();
  if file.read_to_end(&mut appended).is_err() {
    return Vec::new();
  }
  if appended.is_empty() {
    return Vec::new();
  }
  cursor.offset += appended.len() as u64;
  let mut merged = std::mem::take(&mut cursor.carry);
  merged.extend_from_slice(&appended);
  let (entries, consumed) = decode_records(&merged);
  cursor.carry = merged.split_off(consumed);
  entries
}

pub fn list_segments(dir: &Path) -> Vec<PathBuf> {
  if dir.is_file() && dir.extension().is_some_and(|ext| ext == "rlog") {
    return vec![dir.to_path_buf()];
  }
  let mut files = if let Ok(entries) = fs::read_dir(dir) {
    entries
      .filter_map(Result::ok)
      .map(|entry| entry.path())
      .filter(|path| path.extension().is_some_and(|ext| ext == "rlog"))
      .collect::<Vec<_>>()
  } else {
    Vec::new()
  };
  let fallback = PathBuf::from(FALLBACK_LOG_PATH);
  if fallback.is_file() {
    files.push(fallback);
  }
  files.sort();
  files.dedup();
  files
}

pub fn decode_records(data: &[u8]) -> (Vec<LogEntry>, usize) {
  let mut entries = Vec::new();
  let mut cursor = 0usize;
  let cfg = bincode_next::config::standard();
  while cursor + 8 <= data.len() {
    let magic = u32::from_be_bytes(data[cursor..cursor + 4].try_into().unwrap());
    if magic != RLOG_MAGIC {
      cursor += 1;
      continue;
    }
    let total_len = u32::from_be_bytes(data[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
    let frame_end = cursor + 8 + total_len;
    if frame_end > data.len() {
      break;
    }
    if total_len < 8 {
      cursor += 1;
      continue;
    }
    let payload_len =
      u32::from_be_bytes(data[cursor + 8..cursor + 12].try_into().unwrap()) as usize;
    if 4 + payload_len + 4 != total_len {
      cursor += 1;
      continue;
    }
    let payload_start = cursor + 12;
    let payload_end = payload_start + payload_len;
    let crc_start = payload_end;
    let crc_end = crc_start + 4;
    let payload = &data[payload_start..payload_end];
    let crc = u32::from_be_bytes(data[crc_start..crc_end].try_into().unwrap());
    if crc32fast::hash(payload) != crc {
      cursor += 1;
      continue;
    }
    if let Ok((entry, _)) = bincode_next::serde::decode_from_slice::<LogEntry, _>(payload, cfg) {
      entries.push(entry);
    }
    cursor = frame_end;
  }
  (entries, cursor)
}

fn level_rank(level: LogLevel) -> u8 {
  match level {
    LogLevel::Trace => 0,
    LogLevel::Debug => 1,
    LogLevel::Info => 2,
    LogLevel::Warn => 3,
    LogLevel::Error => 4,
    LogLevel::Fatal => 5,
  }
}

pub fn write_log_entry(sink: &mut OutputSink, entry: &LogEntry) -> Result<Void, String> {
  let level = match entry.level {
    LogLevel::Trace => "TRACE".dimmed().to_string(),
    LogLevel::Debug => "DEBUG".bright_blue().to_string(),
    LogLevel::Info => "INFO ".green().bold().to_string(),
    LogLevel::Warn => "WARN ".yellow().bold().to_string(),
    LogLevel::Error => "ERROR".red().bold().to_string(),
    LogLevel::Fatal => "FATAL".on_red().white().bold().to_string(),
  };
  let ts = format_timestamp(entry.timestamp).dimmed().to_string();
  let target = entry.target.blue().bold().to_string();
  sink.line(&format!(
    "[{} {} {}] {}",
    level,
    ts,
    target,
    entry.message.white()
  ))?;
  if !entry.fields.is_empty() {
    let mut fields = entry.fields.iter().collect::<Vec<_>>();
    fields.sort_by(|a, b| a.0.cmp(b.0));
    let rendered = fields
      .into_iter()
      .map(|(k, v)| format!("{}={}", k.cyan(), v.white()))
      .collect::<Vec<_>>()
      .join(" ");
    sink.line(&format!(
      "  {} {}",
      "fields".bold().dimmed(),
      rendered.dimmed()
    ))?;
  }
  Ok(Void)
}

fn format_timestamp(timestamp: u64) -> String {
  unsafe {
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
    std::ffi::CStr::from_ptr(buf.as_ptr() as *const libc::c_char)
      .to_string_lossy()
      .to_string()
  }
}
