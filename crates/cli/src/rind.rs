use std::{
  collections::HashMap,
  fs::{self, File},
  io::{Read, Seek, SeekFrom, Write},
  os::unix::process::CommandExt,
  path::{Path, PathBuf},
  process::{Child, ChildStdin, Command, Stdio},
  thread,
  time::{Duration, SystemTime, UNIX_EPOCH},
};

use clap::{Parser, ValueEnum};
use owo_colors::OwoColorize;
use rind_core::logging::{LogEntry, LogLevel};
use rind_ipc::{
  Message, MessageType,
  payloads::{
    ListPayload, LogoutPayload, PermissionPayload, Run0AuthPayload, SSPayload, ScopeCreatePayload,
    ScopeDestroyPayload,
  },
  send::send_message,
  ser::{
    FacetSerialized, ServiceSerialized, SocketSerialized, UnitItemsSerialized, UnitSerialized,
    flexbuf_string,
  },
};

use flexbuffers;
mod macros;
mod print;

const RLOG_MAGIC: u32 = 0x524C4F47;
const FALLBACK_LOG_PATH: &str = "/var/log/rind-fallback.rlog";

#[derive(clap::Parser)]
#[command(name = "rind")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "Rust Init Daemon")]
struct Cli {
  #[command(subcommand)]
  command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
  Logout,
  Su {
    #[arg(name = "ARGS", trailing_var_arg = true, num_args(1..), allow_hyphen_values = true)]
    args: Vec<String>,
  },
  Logs {
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
  },
  List {
    #[arg(name = "NAME")]
    name: Option<String>,

    #[arg(short = 'u', long)]
    unit: bool,

    #[arg(short = 's', long)]
    service: bool,

    #[arg(short = 'x', long)]
    socket: bool,

    #[arg(short = 'm', long)]
    mount: bool,

    #[arg(short = 'c', long)]
    state: bool,

    #[arg(short = 'n', long)]
    network: bool,

    #[arg(short = 'p', long)]
    port: bool,

    #[arg(short = 't', long)]
    r#type: Option<String>,

    #[arg(long)]
    scope: Option<String>,
  },

  Start {
    #[arg(name = "NAME")]
    name: String,

    #[arg(short = 't', long, default_value = "service")]
    r#type: String,

    #[arg(short = 'p', long)]
    persist: bool,

    #[arg(long)]
    scope: Option<String>,
  },

  Stop {
    #[arg(name = "NAME")]
    name: String,

    #[arg(short = 't', long, default_value = "service")]
    r#type: String,

    #[arg(short = 'f', long)]
    force: bool,

    #[arg(short = 'p', long)]
    persist: bool,

    #[arg(long)]
    scope: Option<String>,
  },

  Permission {
    #[command(subcommand)]
    action: PermissionCommands,
  },

  Invoke {
    #[arg(name = "NAME")]
    name: String,

    #[arg(name = "PAYLOAD")]
    payload: String,

    #[arg(long)]
    scope: Option<String>,
  },
  Scope {
    #[command(subcommand)]
    action: ScopeCommands,
  },
  ReloadUnits,
  SoftReboot,
  Reboot,
  Shutdown,
}

#[derive(clap::Subcommand)]
enum PermissionCommands {
  Grant {
    #[arg(name = "SUBJECT")]
    subject: String,

    #[arg(name = "PERMISSION")]
    permission: String,

    #[arg(short = 'g', long)]
    group: bool,
  },
  Revoke {
    #[arg(name = "SUBJECT")]
    subject: String,

    #[arg(name = "PERMISSION")]
    permission: String,

    #[arg(short = 'g', long)]
    group: bool,
  },
  Show {
    #[arg(long)]
    user: Option<String>,
    #[arg(long)]
    group: Option<String>,
  },
}

#[derive(clap::Subcommand)]
enum ScopeCommands {
  Create {
    #[arg(name = "NAME")]
    name: String,

    #[arg(long)]
    lifetime_state: Option<String>,

    #[arg(long = "attr", value_name = "KEY=VALUE")]
    attrs: Vec<String>,

    #[arg(long)]
    user: Option<String>,
  },
  Destroy {
    #[arg(name = "NAME")]
    name: String,
  },
  List,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogLevelArg {
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
struct LogQuery {
  level: Option<LogLevel>,
  target: Option<String>,
  message: Option<String>,
  since: Option<u64>,
  fields: Vec<(String, String)>,
}

impl LogQuery {
  fn matches(&self, entry: &LogEntry) -> bool {
    if let Some(min_level) = self.level {
      if level_rank(entry.level) < level_rank(min_level) {
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
struct TailCursor {
  offset: u64,
  carry: Vec<u8>,
}

enum OutputSink {
  Stdout,
  Pager { child: Child, stdin: ChildStdin },
}

impl OutputSink {
  fn stdout() -> Self {
    Self::Stdout
  }

  fn less(follow: bool) -> Result<Self, String> {
    let mut cmd = Command::new("less");
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

  fn line(&mut self, line: &str) -> Result<(), String> {
    match self {
      Self::Stdout => {
        println!("{line}");
        Ok(())
      }
      Self::Pager { stdin, .. } => {
        writeln!(stdin, "{line}").map_err(|err| format!("failed to write to less: {err}"))
      }
    }
  }

  fn finish(mut self) -> Result<(), String> {
    match &mut self {
      Self::Stdout => Ok(()),
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

pub fn report_error(msg: &str, err: impl std::fmt::Display) {
  eprintln!("{} {}: {}", "Error".on_red().black(), msg, err);
}

pub fn handle_parse<T>(result: Result<T, String>, payload: String) -> Option<T> {
  match result {
    Err(e) => {
      if e != "Nothing" {
        report_error(&e, payload);
      }
      None
    }
    Ok(e) => Some(e),
  }
}

pub fn handle_run0_message(args: Vec<String>, message: Message) {
  match &message.r#type {
    MessageType::RequestInput => {
      print!("root password: ");
      use std::io::Write;
      std::io::stdout().flush().unwrap();
      let password = rpassword::read_password().unwrap();
      print!("\r\x1b[2K");
      std::io::stdout().flush().unwrap();
      let payload = Run0AuthPayload {
        password: password.trim().to_string(),
      };
      handle_run0_message(
        args,
        match send_message(Message::from("run0").with(flexbuffers::to_vec(&payload).unwrap())) {
          Ok(m) => m,
          Err(e) => {
            report_error("run0 request failed", e);
            return;
          }
        },
      );
    }
    MessageType::Valid => {
      let mut args = args.into_iter();
      let program = args.next().unwrap();

      let mut command = Command::new(program);

      command
        .args(args)
        .gid(0)
        .uid(0)
        .envs(std::env::vars())
        .current_dir(std::env::current_dir().unwrap())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

      let _ = command.status();
    }
    _ => handle_message(message),
  }
}

pub fn handle_message(message: Message) {
  match message.r#type {
    MessageType::Ok => {
      println!(
        "{} {}",
        "Ok".on_green().black(),
        message
          .payload
          .as_ref()
          .map(|p| flexbuf_string(p))
          .unwrap_or_else(|| "ok".to_string())
      );
    }
    MessageType::Error => {
      println!(
        "{} {}",
        "Error".on_red().black(),
        message
          .payload
          .as_ref()
          .map(|p| flexbuf_string(p))
          .unwrap_or_else(|| "unknown error".to_string())
      )
    }
    _ => {}
  }
}

fn apply_scope_name(name: &str, scope: Option<&str>) -> String {
  let Some(scope) = scope else {
    return name.to_string();
  };
  if scope.is_empty() || scope == "static" || name.contains('@') {
    return name.to_string();
  }
  format!("{name}@{scope}")
}

fn parse_scope_attrs(attrs: &[String]) -> Result<HashMap<String, String>, String> {
  let mut out = HashMap::new();
  for attr in attrs {
    let Some((k, v)) = attr.split_once('=') else {
      return Err(format!("invalid --attr value '{attr}', expected KEY=VALUE"));
    };
    let key = k.trim();
    if key.is_empty() {
      return Err(format!(
        "invalid --attr value '{attr}', key cannot be empty"
      ));
    }
    out.insert(key.to_string(), v.trim().to_string());
  }
  Ok(out)
}

fn main() {
  let cli = Cli::parse();

  match cli.command {
    Commands::Su { args } => {
      let output = match send_message(Message::from("run0")) {
        Ok(m) => m,
        Err(e) => {
          report_error("run0 request failed", e);
          return;
        }
      };

      handle_run0_message(args, output);
    }
    Commands::Logout => {
      let username = std::env::var("USER").expect("unknown user");
      let session_id = std::env::var("SESSION_ID")
        .expect("unknown session")
        .parse::<u64>()
        .expect("unknown session");
      handle_send!(
        "logout",
        &LogoutPayload {
          session_id,
          username,
          tty: None
        }
      );
    }
    Commands::Logs {
      dir,
      level,
      target,
      message,
      since,
      current,
      fields,
      limit,
      tail,
      less,
      poll_ms,
    } => {
      let since = match resolve_since(since, current) {
        Ok(v) => v,
        Err(err) => {
          report_error("invalid logs query", err);
          return;
        }
      };

      let query = match build_log_query(level, target, message, since, fields) {
        Ok(query) => query,
        Err(err) => {
          report_error("invalid logs query", err);
          return;
        }
      };

      let mut sink = match if less {
        OutputSink::less(tail)
      } else {
        Ok(OutputSink::stdout())
      } {
        Ok(sink) => sink,
        Err(err) => {
          report_error("logs output setup failed", err);
          return;
        }
      };

      if tail {
        if let Err(err) = tail_logs(dir.as_path(), &query, poll_ms, limit, &mut sink) {
          report_error("logs tail failed", err);
        }
      } else {
        let entries = read_entries_once(dir.as_path(), &query, limit);
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
            dir.display()
          );
        }

        if let Err(err) = sink.finish() {
          report_error("logs output failed", err);
        }
      }
    }
    Commands::List {
      unit,
      service,
      mount,
      state,
      socket,
      network,
      port,
      mut r#type,
      name,
      scope,
    } => {
      let name = apply_scope_name(&name.unwrap_or_default(), scope.as_deref());
      let result = send_msg!(
        "list",
        flexbuffers::to_vec(&ListPayload {
          name: name.clone().into(),
          unit_type: if unit {
            "unit"
          } else if service {
            "service"
          } else if mount {
            "mount"
          } else if state {
            "state"
          } else if socket {
            "socket"
          } else if port && network {
            "netport"
          } else if network {
            "netiface"
          } else if r#type.is_some() {
            r#type.as_ref().unwrap()
          } else {
            "unknown"
          }
          .into(),
        })
        .unwrap()
      )
      .expect("Failed to send message");

      if r#type.is_none() && port && network {
        r#type = Some("netports".to_string());
      } else if r#type.is_none() && network {
        r#type = Some("netiface".to_string());
      }

      if matches!(result.r#type, MessageType::Error) {
        handle_message(result);
        return;
      }

      if unit {
        print::print_unit(
          &name,
          &result
            .parse_payload::<UnitItemsSerialized>()
            .expect("Failed to parse"),
        );
      } else if service {
        print::print_service(
          &result
            .parse_payload::<ServiceSerialized>()
            .expect("Failed to parse"),
        );

        let entries = read_entries_once(
          &PathBuf::from("/var/log/rind"),
          &LogQuery {
            level: None,
            target: None,
            message: None,
            since: None,
            fields: vec![("service".to_string(), name.clone())],
          },
          10,
        );
        for entry in &entries {
          if let Err(err) = write_log_entry(&mut OutputSink::stdout(), entry) {
            report_error("logs print failed", err);
            return;
          }
        }
      } else if state {
        print::print_state(
          &result
            .parse_payload::<FacetSerialized>()
            .expect("Failed to parse"),
        );
      } else if socket {
        print::print_socket(
          &result
            .parse_payload::<SocketSerialized>()
            .expect("Failed to parse"),
        );
      } else if r#type.is_some()
        && let Some(ref ty) = r#type
        && !ty.is_empty()
        && ty != "unknown"
      {
        if let Ok(list) = result.parse_payload::<rind_ipc::ser::IpcListComponent>() {
          print::print_ipc_list(&list);
        } else {
          println!(
            "{}",
            String::from_utf8_lossy(&result.payload.unwrap_or_default())
          );
        }
      } else {
        print::print_units(
          &result
            .parse_vec_payload::<UnitSerialized>()
            .expect("Failed to parse"),
        );
      }
    }
    Commands::Invoke {
      name,
      payload,
      scope,
    } => {
      let mut v: serde_json::Value =
        serde_json::from_str(&payload).unwrap_or(serde_json::Value::String(payload));

      if let Some(scope) = scope.filter(|s| !s.is_empty() && s != "static") {
        if let Some(obj) = v.as_object_mut()
          && let Some(n) = obj.get("name").and_then(|x| x.as_str())
        {
          obj.insert(
            "name".to_string(),
            serde_json::Value::String(apply_scope_name(n, Some(&scope))),
          );
        }
      }

      handle_send_raw!(name.as_str(), flexbuffers::to_vec(&v).unwrap());
    }
    Commands::Scope { action } => match action {
      ScopeCommands::Create {
        name,
        lifetime_state,
        attrs,
        user,
      } => {
        let mut attributes = match parse_scope_attrs(&attrs) {
          Ok(v) => v,
          Err(err) => {
            report_error("invalid scope attributes", err);
            return;
          }
        };
        if let Some(user) = user {
          attributes.insert("user".to_string(), user);
        }
        handle_send!(
          "create_scope",
          &ScopeCreatePayload {
            scope: name,
            lifetime_state,
            attributes,
          }
        );
      }
      ScopeCommands::Destroy { name } => {
        handle_send!("destroy_scope", &ScopeDestroyPayload { scope: name });
      }
      ScopeCommands::List => {
        let result = send_msg!("list_scopes", Vec::new()).expect("Failed to send message");

        if let Ok(list) = result.parse_payload::<rind_ipc::ser::IpcListComponent>() {
          print::print_ipc_list(&list);
        } else {
          println!(
            "{}",
            String::from_utf8_lossy(&result.payload.unwrap_or_default())
          );
        }
      }
    },
    Commands::Permission { action } => match action {
      PermissionCommands::Grant {
        subject,
        permission,
        group,
      } => {
        handle_send!(
          "grant_permission",
          &PermissionPayload {
            group,
            permission,
            subject
          }
        );
      }
      PermissionCommands::Revoke {
        subject,
        permission,
        group,
      } => {
        handle_send!(
          "revoke_permission",
          &PermissionPayload {
            group,
            permission,
            subject
          }
        );
      }
      PermissionCommands::Show { user, group } => {
        let result = send_msg!(
          "show_permissions",
          if let Some(user) = user {
            flexbuffers::to_vec(PermissionPayload {
              subject: user,
              group: false,
              permission: String::new(),
            })
            .unwrap()
          } else if let Some(group) = group {
            flexbuffers::to_vec(PermissionPayload {
              subject: group,
              group: true,
              permission: String::new(),
            })
            .unwrap()
          } else {
            Vec::new()
          }
        )
        .expect("Failed to send message");

        if let Ok(list) = result.parse_payload::<rind_ipc::ser::IpcListComponent>() {
          print::print_ipc_list(&list);
        } else {
          println!(
            "{}",
            String::from_utf8_lossy(&result.payload.unwrap_or_default())
          );
        }
      }
    },
    Commands::ReloadUnits => {
      handle_send!("reload_units", &());
    }
    Commands::SoftReboot => {
      handle_send!("soft_reboot", &());
    }
    Commands::Reboot => {
      handle_send!("reboot", &());
    }
    Commands::Shutdown => {
      handle_send!("shutdown", &());
    }
    Commands::Start {
      name,
      r#type,
      persist,
      scope,
    } => {
      let name = apply_scope_name(&name, scope.as_deref());
      let action = if r#type == "socket" || r#type == "soc" {
        "start_socket"
      } else if r#type == "service" || r#type == "svc" {
        "start_service"
      } else {
        "start"
      };
      handle_send!(
        action,
        &SSPayload {
          force: false,
          name,
          persist,
          unit_type: r#type
        }
      );
    }
    Commands::Stop {
      name,
      r#type,
      force,
      persist,
      scope,
    } => {
      let name = apply_scope_name(&name, scope.as_deref());
      let action = if r#type == "socket" || r#type == "soc" {
        "stop_socket"
      } else if r#type == "service" || r#type == "svc" {
        "stop_service"
      } else {
        "stop"
      };
      handle_send!(
        action,
        &SSPayload {
          force,
          name,
          persist,
          unit_type: r#type
        }
      );
    }
  }
}

fn build_log_query(
  level: Option<LogLevelArg>,
  target: Option<String>,
  message: Option<String>,
  since: Option<u64>,
  raw_fields: Vec<String>,
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
    level: level.map(Into::into),
    target,
    message,
    since,
    fields,
  })
}

fn resolve_since(since: Option<u64>, current: bool) -> Result<Option<u64>, String> {
  if !current {
    return Ok(since);
  }

  let boot_start = current_boot_start_unix()?;
  Ok(Some(since.map(|s| s.max(boot_start)).unwrap_or(boot_start)))
}

fn current_boot_start_unix() -> Result<u64, String> {
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

fn read_entries_once(dir: &Path, query: &LogQuery, limit: usize) -> Vec<LogEntry> {
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
) -> Result<(), String> {
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

fn list_segments(dir: &Path) -> Vec<PathBuf> {
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

fn decode_records(data: &[u8]) -> (Vec<LogEntry>, usize) {
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

fn write_log_entry(sink: &mut OutputSink, entry: &LogEntry) -> Result<(), String> {
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

  Ok(())
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
