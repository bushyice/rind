use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rind_ipc::Message;
use rind_ipc::payloads::ServicePayload;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::ops::{Deref, DerefMut};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use rind_core::prelude::*;

use crate::flow::{FlowInstance, FlowItem, FlowPayload, FlowType, StateMachine, Trigger};
use crate::permissions::PERM_SYSTEM_SERVICES;
use crate::transport::{TransportMessage, TransportMethod, start_stdout_listener};
use crate::variables::VariableHeap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RunOption {
  #[serde(default)]
  pub exec: String,
  #[serde(default)]
  pub args: Vec<String>,
  pub env: Option<HashMap<String, String>>,
  pub variable: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum RunOptions {
  One(RunOption),
  Many(Vec<RunOption>),
}

impl RunOptions {
  pub fn as_one(&self) -> &RunOption {
    match self {
      RunOptions::One(k) => k,
      RunOptions::Many(k) => k.first().unwrap(),
    }
  }

  pub fn as_many(&self) -> impl Iterator<Item = &RunOption> {
    match self {
      RunOptions::One(k) => std::slice::from_ref(k).iter(),
      RunOptions::Many(k) => k.iter(),
    }
  }

  pub fn to_string(&self) -> Vec<String> {
    self
      .as_many()
      .map(|x| format!("{} {}", x.exec, x.args.join(" ")))
      .collect::<Vec<String>>()
  }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum RestartPolicy {
  Bool(bool),
  OnFailure { max_retries: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMode {
  Graceful,
  ForceKill,
}

impl Default for RestartPolicy {
  fn default() -> Self {
    Self::Bool(false)
  }
}

static SERVICE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ServiceId(u64);

impl Default for ServiceId {
  fn default() -> Self {
    Self(SERVICE_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
  }
}

#[derive(Default, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ServiceState {
  Active,
  #[default]
  Inactive,
  Starting,
  Stopping,
  Exited(i32),
  Error(String),
}

pub struct ChildInstance {
  pub key: String,
  pub user: Option<String>,
  pub child: Option<Child>,
  pub state: ServiceState,
  pub retry_count: u32,
  pub stop_time: Option<Instant>,
  pub manually_stopped: bool,
}

impl ChildInstance {
  pub fn new(key: String, user: Option<String>, child: Option<Child>) -> Self {
    Self {
      key,
      user,
      child,
      state: ServiceState::Active,
      retry_count: 0,
      stop_time: None,
      manually_stopped: false,
    }
  }

  pub fn pid(&self) -> Option<u32> {
    self.child.as_ref().map(|c| c.id())
  }
}

#[derive(Default)]
pub struct ChildInstanceGroup(pub Vec<ChildInstance>);

impl ChildInstanceGroup {
  pub fn as_one(&self) -> Option<&ChildInstance> {
    self.0.first()
  }

  pub fn as_one_mut(&mut self) -> Option<&mut ChildInstance> {
    self.0.first_mut()
  }

  pub fn find_by_pid(&self, pid: i32) -> Option<usize> {
    self
      .0
      .iter()
      .position(|inst| inst.child.as_ref().map(|c| c.id() as i32) == Some(pid))
  }

  pub fn is_active(&self) -> bool {
    self.0.iter().any(|x| x.state == ServiceState::Active)
  }

  pub fn pid(&self) -> Vec<u32> {
    self.0.iter().filter_map(|x| x.pid()).collect()
  }

  pub fn last_state(&self) -> String {
    self
      .0
      .last()
      .map_or("Inactive".to_string(), |x| format!("{:?}", x.state))
  }
}

impl Deref for ChildInstanceGroup {
  type Target = Vec<ChildInstance>;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for ChildInstanceGroup {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BranchingConfig {
  #[serde(default)]
  pub enabled: bool,
  #[serde(rename = "source-state")]
  pub source_state: String,
  #[serde(default)]
  pub key: Option<String>,
  #[serde(rename = "max-instances", default)]
  pub max_instances: Option<usize>,
}

fn default_username_field() -> String {
  "username".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceUserSource {
  pub state: String,
  #[serde(rename = "username-field", default = "default_username_field")]
  pub username_field: String,
  #[serde(rename = "match-branch-key")]
  pub match_branch_key: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceSpace {
  #[default]
  System,
  User,
  UserSelective {
    user: String,
  },
}

#[model(
  meta_name = name,
  meta_fields(
    name, run, after, branching, restart, start_on, stop_on, on_start, on_stop,
    transport, working_dir, space, user_source
  ),
  derive_metadata(Debug)
)]
pub struct Service {
  // Metadata
  pub name: String,
  pub run: RunOptions,
  pub after: Option<Vec<String>>,
  #[serde(rename = "start-on")]
  pub start_on: Option<Vec<FlowItem>>,
  #[serde(rename = "stop-on")]
  pub stop_on: Option<Vec<FlowItem>>,
  #[serde(rename = "on-start")]
  pub on_start: Option<Vec<Trigger>>,
  #[serde(rename = "on-stop")]
  pub on_stop: Option<Vec<Trigger>>,
  #[serde(rename = "working-dir")]
  pub working_dir: Option<String>,
  #[serde(default, rename = "space")]
  pub space: ServiceSpace,
  #[serde(rename = "user-source")]
  pub user_source: Option<ServiceUserSource>,
  pub transport: Option<TransportMethod>,
  pub branching: Option<BranchingConfig>,
  pub restart: Option<RestartPolicy>,

  // Instance data
  pub id: ServiceId,
  pub instances: ChildInstanceGroup,
}

impl Service {
  pub fn new(metadata: Arc<ServiceMetadata>) -> Self {
    Self {
      metadata,
      id: ServiceId::default(),
      instances: ChildInstanceGroup::default(),
    }
  }
}

pub struct ServiceRuntime {
  event_rx: Option<rind_core::events::Subscription<rind_core::prelude::FlowEvent>>,
  stdio_tx: Sender<(String, TransportMessage)>,
  stdio_rx: Receiver<(String, TransportMessage)>,
  stdio_writers: Mutex<HashMap<String, Vec<Sender<TransportMessage>>>>,
}

impl Default for ServiceRuntime {
  fn default() -> Self {
    let (stdio_tx, stdio_rx) = mpsc::channel();
    Self {
      event_rx: None,
      stdio_tx,
      stdio_rx,
      stdio_writers: Mutex::new(HashMap::new()),
    }
  }
}

impl ServiceRuntime {
  fn payload_field_as_key(payload: &FlowPayload, field: &str) -> Option<String> {
    payload.get_json_field(field).map(|v| {
      if let Some(s) = v.as_str() {
        s.to_string()
      } else {
        v.to_string()
      }
    })
  }

  fn branch_key_from_payload(payload: &FlowPayload, key_name: Option<&str>) -> Option<String> {
    if let Some(key_name) = key_name {
      return Self::payload_field_as_key(payload, key_name).filter(|v| !v.is_empty());
    }
    let value = payload.to_string_payload();
    if value.is_empty() { None } else { Some(value) }
  }

  fn resolve_user_from_source(
    &self,
    source: &ServiceUserSource,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&StateMachine>,
  ) -> anyhow::Result<Option<String>> {
    let Some(sm) = sm else {
      return Ok(None);
    };
    let Some(branches) = sm.states.get(&source.state) else {
      return Ok(None);
    };

    if let Some(field) = &source.match_branch_key {
      let Some(expected) = branch_ctx.and_then(|ctx| ctx.key.as_deref()) else {
        return Ok(None);
      };
      let mut matches = HashSet::new();
      for branch in branches {
        let Some(found) = Self::payload_field_as_key(&branch.payload, field) else {
          continue;
        };
        if found != expected {
          continue;
        }
        if let Some(user) = Self::payload_field_as_key(&branch.payload, &source.username_field) {
          matches.insert(user);
        }
      }
      if matches.is_empty() {
        return Ok(None);
      }
      if matches.len() > 1 {
        return Err(anyhow::anyhow!(
          "ambiguous users for state '{}' using match key '{}'",
          source.state,
          field
        ));
      }
      return Ok(matches.into_iter().next());
    }

    if let Some(payload) = branch_ctx.and_then(|ctx| ctx.payload.as_ref())
      && let Some(user) = Self::payload_field_as_key(payload, &source.username_field)
    {
      return Ok(Some(user));
    }

    let mut users = HashSet::new();
    for branch in branches {
      if let Some(user) = Self::payload_field_as_key(&branch.payload, &source.username_field) {
        users.insert(user);
      }
    }
    if users.is_empty() {
      return Ok(None);
    }
    if users.len() > 1 {
      return Err(anyhow::anyhow!(
        "ambiguous users in state '{}' (set user-source.match-branch-key)",
        source.state
      ));
    }
    Ok(users.into_iter().next())
  }

  fn resolve_service_user(
    &self,
    service: &Service,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&StateMachine>,
  ) -> anyhow::Result<Option<String>> {
    if let Some(user) = branch_ctx.and_then(|ctx| ctx.forced_user.as_ref()) {
      return Ok(Some(user.clone()));
    }

    match &service.metadata.space {
      ServiceSpace::System => Ok(None),
      ServiceSpace::UserSelective { user } => Ok(Some(user.clone())),
      ServiceSpace::User => {
        if let Some(source) = &service.metadata.user_source
          && let Some(user) = self.resolve_user_from_source(source, branch_ctx, sm)?
        {
          return Ok(Some(user));
        }

        if let Some(user) = branch_ctx.and_then(|ctx| ctx.key.as_ref()) {
          return Ok(Some(user.clone()));
        }

        if let Some(sm) = sm
          && let Some(sessions) = sm.states.get("rind@user_session")
        {
          let mut users = HashSet::new();
          for sess in sessions {
            if let Some(user) = Self::payload_field_as_key(&sess.payload, "username") {
              users.insert(user);
            }
          }
          if users.len() == 1 {
            return Ok(users.into_iter().next());
          }
          if users.len() > 1 {
            return Err(anyhow::anyhow!(
              "service '{}' is userspace but username is ambiguous; configure `user-source`",
              service.metadata.name
            ));
          }
        }

        Ok(None)
      }
    }
  }

  pub fn spawn_all(
    &self,
    service: &Service,
    log: &LogHandle,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&StateMachine>,
    variable_heap: Option<&VariableHeap>,
  ) -> anyhow::Result<Vec<ChildInstance>> {
    service
      .metadata
      .run
      .as_many()
      .map(|run| {
        let resolved = self.resolve_run_option(run, variable_heap);
        let run_ref = resolved.as_ref().unwrap_or(run);
        self.spawn_process(service, run_ref, log, branch_ctx, sm)
      })
      .collect()
  }

  fn resolve_run_option(
    &self,
    run: &RunOption,
    variable_heap: Option<&VariableHeap>,
  ) -> Option<RunOption> {
    let var_ref = run.variable.as_deref()?;
    let heap = variable_heap?;

    let value = heap.get(var_ref)?;

    let table = value.as_table()?;
    let exec = table
      .get("exec")
      .and_then(|v| v.as_str())
      .unwrap_or_default()
      .to_string();
    let args = table
      .get("args")
      .and_then(|v| v.as_array())
      .map(|arr| {
        arr
          .iter()
          .filter_map(|v| v.as_str().map(|s| s.to_string()))
          .collect()
      })
      .unwrap_or_default();
    let env = table.get("env").and_then(|v| v.as_table()).map(|t| {
      t.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
    });

    Some(RunOption {
      exec,
      args,
      env,
      variable: None,
    })
  }

  pub fn spawn_service(
    &self,
    service: &mut Service,
    log: &LogHandle,
    sm: Option<&StateMachine>,
    variable_heap: Option<&VariableHeap>,
  ) -> anyhow::Result<()> {
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service started",
      self.log_fields(service, "start"),
    );

    let instances = self.spawn_all(service, log, None, sm, variable_heap)?;
    service.instances.extend(instances);
    Ok(())
  }

  fn log_fields(&self, service: &Service, action: impl Into<String>) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    fields.insert("service".to_string(), service.metadata.name.clone());
    fields.insert("action".to_string(), action.into());
    fields
  }

  fn spawn_process(
    &self,
    service: &Service,
    run: &RunOption,
    log: &LogHandle,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&StateMachine>,
  ) -> anyhow::Result<ChildInstance> {
    let mut args = run.args.clone();
    let mut envs = run.env.clone().unwrap_or_default();
    let branch_key = branch_ctx.and_then(|ctx| ctx.key.as_deref());
    let resolved_user = self.resolve_service_user(service, branch_ctx, sm)?;

    if let Some(transport) = &service.metadata.transport {
      if let Some(sm) = sm {
        let resolve_state = |spec: &str| -> Option<String> {
          let (state_name, path) = spec
            .split_once('/')
            .map(|(name, p)| (name, Some(p)))
            .unwrap_or((spec, None));
          let payload = sm
            .states
            .get(state_name)
            .and_then(|v| v.first())
            .map(|x| x.payload.clone())?;
          let Some(path) = path else {
            return Some(payload.to_string_payload());
          };

          match payload {
            FlowPayload::Json(json) => {
              let mut cur = json.into_json();
              for key in path.split('.') {
                cur = cur.get(key)?.clone();
              }
              if let Some(s) = cur.as_str() {
                Some(s.to_string())
              } else {
                Some(cur.to_string())
              }
            }
            FlowPayload::String(s) => Some(s),
            FlowPayload::Bytes(b) => Some(String::from_utf8(b).unwrap_or_default()),
            FlowPayload::None(_) => Some(String::new()),
          }
        };

        match transport {
          crate::transport::TransportMethod::Options {
            id,
            options,
            permissions: _,
          } if id.0 == "env" => {
            for option in options {
              let Some((key, value)) = option.split_once('=') else {
                continue;
              };
              if let Some(state_name) = value.strip_prefix("state:") {
                if let Some(val) = resolve_state(state_name) {
                  envs.insert(key.to_string(), val);
                }
              } else {
                envs.insert(key.to_string(), value.to_string());
              }
            }
          }
          crate::transport::TransportMethod::Options {
            id,
            options,
            permissions: _,
          } if id.0 == "args" => {
            for option in options {
              if let Some(state_name) = option.strip_prefix("state:") {
                let payload = resolve_state(state_name).unwrap_or_default();
                if !payload.is_empty() {
                  args.push(payload);
                }
              } else {
                args.push(option.clone());
              }
            }
          }
          _ => {}
        }
      }
    }

    let child = unsafe {
      let mut cmd = Command::new(&run.exec);
      cmd
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .pre_exec(|| {
          libc::setsid();
          Ok(())
        });
      let user_info = if let Some(username) = resolved_user.as_deref() {
        let store = rind_core::user::UserStore::load_system()
          .map_err(|e| anyhow::anyhow!("failed to load user store: {e}"))?;
        let Some(user) = store.lookup_by_name(username) else {
          return Err(anyhow::anyhow!(
            "user '{}' not found for service '{}'",
            username,
            service.metadata.name
          ));
        };
        Some((user.uid, user.gid, user.home.clone(), username.to_string()))
      } else {
        None
      };

      if let Some(dir) = &service.metadata.working_dir {
        cmd.current_dir(dir);
      }

      if matches!(service.metadata.space, ServiceSpace::User) && user_info.is_none() {
        return Err(anyhow::anyhow!(
          "failed to resolve userspace identity for '{}'",
          service.metadata.name
        ));
      }

      if let Some((uid, gid, home, username)) = user_info {
        cmd.uid(uid);
        cmd.gid(gid);

        if let Some(dir) = &service.metadata.working_dir
          && dir.starts_with("~")
        {
          cmd.current_dir(format!("{}{}", home, &dir[1..]));
        }

        envs.insert("HOME".to_string(), home);
        envs.insert("USER".to_string(), username);
      }

      if let Some(key) = branch_key {
        cmd.env("RIND_BRANCH_KEY", key);
      }
      if !envs.is_empty() {
        cmd.envs(&envs);
      }
      cmd.spawn()?
    };

    let mut child = child;
    if service
      .metadata
      .transport
      .as_ref()
      .map(is_stdio_transport)
      .unwrap_or(false)
    {
      start_stdout_listener(
        service.metadata.name.clone(),
        &mut child,
        self.stdio_tx.clone(),
      );
      let (tx, rx) = mpsc::channel::<TransportMessage>();
      start_stdin_writer(service.metadata.name.clone(), &mut child, log.clone(), rx);
      if let Ok(mut writers) = self.stdio_writers.lock() {
        writers
          .entry(service.metadata.name.clone())
          .or_default()
          .push(tx);
      }
    } else {
      start_service_stream_logs(service.metadata.name.clone(), &mut child, log.clone());
    }

    Ok(ChildInstance::new(
      branch_key.map(|x| x.to_string()).unwrap_or_default(),
      resolved_user,
      Some(child),
    ))
  }

  pub fn start_service(
    &self,
    service: &mut Service,
    log: &LogHandle,
    sm: Option<&StateMachine>,
    dispatch: &RuntimeDispatcher,
    variable_heap: Option<&VariableHeap>,
  ) {
    if let Some(inst) = service.instances.as_one() {
      if inst.state == ServiceState::Active || inst.state == ServiceState::Starting {
        return;
      }
    }

    match self.spawn_service(service, log, sm, variable_heap) {
      Ok(_) => {
        self.register_stdio_transport(service, dispatch);
        if let Some(inst) = service.instances.as_one_mut() {
          inst.state = ServiceState::Active;
          self.run_triggers(service.metadata.on_start.as_ref(), dispatch);
        }

        let _ = dispatch.dispatch(
          "services",
          "reconcile_stacks",
          json!({
            "service": service.metadata.name,
            "id": service.id.0,
            "action": ServiceEventKind::Started
          })
          .into(),
        );
      }
      Err(e) => {
        let err = format!("Failed to start service \"{}\": {e}", service.metadata.name);
        let mut fields = self.log_fields(service, "start");
        fields.insert("error".into(), e.to_string());
        log.log(
          LogLevel::Error,
          "service-runtime",
          "failed to start service",
          fields,
        );
        if let Some(inst) = service.instances.as_one_mut() {
          inst.state = ServiceState::Error(err);
        }
      }
    }
  }

  pub fn stop_service(
    &self,
    service: &mut Service,
    mode: StopMode,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    key: Option<String>,
    user: Option<String>,
  ) {
    for inst in service.instances.iter_mut() {
      if let Some(ref key) = key {
        if &inst.key != key {
          continue;
        }
      };
      if let Some(ref user) = user {
        let matches_owner = inst.user.as_ref().map(|u| u == user).unwrap_or(false)
          || (inst.user.is_none() && &inst.key == user);
        if !matches_owner {
          continue;
        }
      }
      if let Some(child) = inst.child.as_ref() {
        let pgid = Pid::from_raw(-(child.id() as i32));
        let signal = if mode == StopMode::ForceKill {
          Signal::SIGKILL
        } else {
          Signal::SIGTERM
        };
        let _ = kill(pgid, signal);
        inst.state = ServiceState::Stopping;
        inst.stop_time = Some(Instant::now());
        inst.manually_stopped = true;
      } else {
        if inst.state == ServiceState::Active {
          self.run_triggers(service.metadata.on_stop.as_ref(), dispatch);
        }
        inst.state = ServiceState::Inactive;
      }
    }

    let mut fields = self.log_fields(service, "stop");
    fields.insert("mode".to_string(), format!("{mode:?}"));
    if let Some(ref key) = key {
      fields.insert("key".to_string(), format!("{key}"));
    };
    if let Some(ref user) = user {
      fields.insert("user".to_string(), user.clone());
    };
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service stopping",
      fields,
    );
    let _ = dispatch.dispatch(
      "services",
      "reconcile_stacks",
      json!({
        "service": service.metadata.name,
        "id": service.id.0,
        "action": ServiceEventKind::Stopped
      })
      .into(),
    );
  }

  fn handle_child_exit(
    &self,
    service: &mut Service,
    pid: i32,
    code: i32,
    _log: &LogHandle,
    dispatch: &RuntimeDispatcher,
  ) -> Option<ServiceExitAction> {
    let idx = service.instances.find_by_pid(pid)?;
    let (manually_stopped, retry_count) = {
      let inst = &mut service.instances.0[idx];

      if matches!(inst.state, ServiceState::Active | ServiceState::Stopping) {
        self.run_triggers(service.metadata.on_stop.as_ref(), dispatch);
      }

      inst.state = ServiceState::Exited(code);
      inst.child = None;
      (inst.manually_stopped, inst.retry_count)
    };

    self.maybe_unregister_stdio_transport(service, dispatch);

    if manually_stopped {
      return Some(ServiceExitAction::StopDependents);
    }

    let restart_policy = service.metadata.restart.as_ref();
    match restart_policy {
      Some(RestartPolicy::Bool(true)) => Some(ServiceExitAction::Restart),
      Some(RestartPolicy::OnFailure { max_retries }) => {
        if code != 0 && *max_retries > 0 && retry_count < *max_retries {
          if let Some(inst) = service.instances.0.get_mut(idx) {
            inst.retry_count += 1;
          }
          Some(ServiceExitAction::Restart)
        } else {
          Some(ServiceExitAction::StopDependents)
        }
      }
      _ => Some(ServiceExitAction::StopDependents),
    }
  }

  fn timeout_sweep(&self, service: &mut Service) {
    for inst in service.instances.iter_mut() {
      if inst.state == ServiceState::Stopping {
        if let Some(stop_time) = inst.stop_time {
          if stop_time.elapsed() > std::time::Duration::from_secs(5) {
            if let Some(child) = inst.child.as_ref() {
              let pgid = Pid::from_raw(-(child.id() as i32));
              let _ = kill(pgid, Signal::SIGKILL);
            }
          }
        }
      }
    }
  }

  fn run_triggers(&self, triggers: Option<&Vec<Trigger>>, dispatch: &RuntimeDispatcher) {
    if let Some(triggers) = triggers {
      for trigger in triggers {
        if let Some(script) = &trigger.script {
          let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .spawn();
        } else if let Some(exec) = &trigger.exec {
          let mut cmd = std::process::Command::new(exec);
          if let Some(args) = &trigger.args {
            cmd.args(args);
          }
          let _ = cmd.spawn();
        } else if let Some(state) = &trigger.state {
          let mut payload = serde_json::json!({ "name": state });
          if let Some(p) = &trigger.payload {
            payload["payload"] = p.clone();
          }
          let _ = dispatch.dispatch("flow", "set_state", payload.into());
        } else if let Some(signal) = &trigger.signal {
          let mut payload = serde_json::json!({ "name": signal });
          if let Some(p) = &trigger.payload {
            payload["payload"] = p.clone();
          }
          let _ = dispatch.dispatch("flow", "emit_signal", payload.into());
        }
      }
    }
  }

  fn register_stdio_transport(&self, service: &Service, dispatch: &RuntimeDispatcher) {
    if !service
      .metadata
      .transport
      .as_ref()
      .map(is_stdio_transport)
      .unwrap_or(false)
    {
      return;
    }
    let _ = dispatch.dispatch(
      "transport",
      "register_stdio",
      serde_json::json!({ "endpoint": service.metadata.name }).into(),
    );
  }

  fn maybe_unregister_stdio_transport(&self, service: &Service, dispatch: &RuntimeDispatcher) {
    if !service
      .metadata
      .transport
      .as_ref()
      .map(is_stdio_transport)
      .unwrap_or(false)
    {
      return;
    }
    let active = service.instances.iter().any(|inst| inst.child.is_some());
    if active {
      return;
    }
    let _ = dispatch.dispatch(
      "transport",
      "unregister_stdio",
      serde_json::json!({ "endpoint": service.metadata.name }).into(),
    );
  }

  fn send_stdio_message(&self, endpoint: &str, message: TransportMessage) {
    let Ok(mut writers) = self.stdio_writers.lock() else {
      return;
    };
    let Some(entries) = writers.get_mut(endpoint) else {
      return;
    };
    entries.retain(|tx| tx.send(message.clone()).is_ok());
    if entries.is_empty() {
      writers.remove(endpoint);
    }
  }

  fn broadcast_stdio_event(&self, event: &rind_core::prelude::FlowEvent) {
    let Ok(mut writers) = self.stdio_writers.lock() else {
      return;
    };

    let msg = TransportMessage {
      r#type: match event.flow_type {
        rind_core::prelude::FlowEventType::State => crate::transport::TransportMessageType::State,
        rind_core::prelude::FlowEventType::Signal => crate::transport::TransportMessageType::Signal,
      },
      payload: Some(FlowPayload::from_json(Some(event.payload.clone()))),
      branch: None,
      name: Some(event.name.clone()),
      action: if event.action == FlowAction::Revert {
        crate::transport::TransportMessageAction::Remove
      } else {
        crate::transport::TransportMessageAction::Set
      },
    };

    for entries in writers.values_mut() {
      entries.retain(|tx| tx.send(msg.clone()).is_ok());
    }
    writers.retain(|_, entries| !entries.is_empty());
  }

  fn stdio_log_entry(
    &self,
    service_name: &str,
    message: &TransportMessage,
  ) -> (LogLevel, String, HashMap<String, String>) {
    let mut level = LogLevel::Info;
    let mut text = String::new();
    let mut fields = HashMap::new();
    fields.insert("service".to_string(), service_name.to_string());
    fields.insert("source".to_string(), "stdio".to_string());

    if let Some(payload) = message.payload.as_ref() {
      match payload {
        FlowPayload::String(s) => {
          text = s.clone();
        }
        FlowPayload::Bytes(b) => {
          text = String::from_utf8(b.clone()).unwrap_or_default();
        }
        FlowPayload::Json(json) => {
          let value = json.into_json();
          if let Some(s) = value.get("message").and_then(|v| v.as_str()) {
            text = s.to_string();
          } else {
            text = value.to_string();
          }

          if let Some(lvl) = value.get("level").and_then(|v| v.as_str()) {
            level = parse_log_level(lvl);
          }

          if let Some(extra) = value.get("fields").and_then(|v| v.as_object()) {
            for (k, v) in extra {
              let val = v
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| v.to_string());
              fields.insert(k.clone(), val);
            }
          }
        }
        FlowPayload::None(_) => {}
      }
    }

    if text.is_empty() {
      text = "log".to_string();
    }

    (level, text, fields)
  }
}

enum ServiceExitAction {
  Restart,
  StopDependents,
}

fn is_stdio_transport(method: &TransportMethod) -> bool {
  match method {
    TransportMethod::Type(id) => id.0 == "stdio",
    TransportMethod::Options { id, .. } => id.0 == "stdio",
    TransportMethod::Object { id, .. } => id.0 == "stdio",
  }
}

fn parse_log_level(input: &str) -> LogLevel {
  match input.to_ascii_lowercase().as_str() {
    "trace" => LogLevel::Trace,
    "debug" => LogLevel::Debug,
    "warn" | "warning" => LogLevel::Warn,
    "error" => LogLevel::Error,
    "fatal" => LogLevel::Fatal,
    _ => LogLevel::Info,
  }
}

fn start_service_stream_logs(service_name: String, child: &mut Child, log: LogHandle) {
  if let Some(stdout) = child.stdout.take() {
    let service_name = service_name.clone();
    let log = log.clone();
    std::thread::spawn(move || {
      let reader = BufReader::new(stdout);
      for line in reader.lines().map_while(std::result::Result::ok) {
        if line.trim().is_empty() {
          continue;
        }
        let mut fields = HashMap::new();
        fields.insert("service".to_string(), service_name.clone());
        fields.insert("stream".to_string(), "stdout".to_string());
        log.log(LogLevel::Info, "service-output", line, fields);
      }
    });
  }

  if let Some(stderr) = child.stderr.take() {
    std::thread::spawn(move || {
      let reader = BufReader::new(stderr);
      for line in reader.lines().map_while(std::result::Result::ok) {
        if line.trim().is_empty() {
          continue;
        }
        let mut fields = HashMap::new();
        fields.insert("service".to_string(), service_name.clone());
        fields.insert("stream".to_string(), "stderr".to_string());
        log.log(LogLevel::Warn, "service-output", line, fields);
      }
    });
  }
}

fn start_stdin_writer(
  service_name: String,
  child: &mut Child,
  log: LogHandle,
  rx: Receiver<TransportMessage>,
) {
  let Some(mut stdin) = child.stdin.take() else {
    return;
  };

  std::thread::spawn(move || {
    while let Ok(msg) = rx.recv() {
      let Ok(frame) = serde_json::to_string(&msg) else {
        continue;
      };

      if std::io::Write::write_all(&mut stdin, frame.as_bytes()).is_err()
        || std::io::Write::write_all(&mut stdin, b"\n").is_err()
        || std::io::Write::flush(&mut stdin).is_err()
      {
        let mut fields = HashMap::new();
        fields.insert("service".to_string(), service_name.clone());
        log.log(
          LogLevel::Warn,
          "service-transport",
          "stdio egress failed",
          fields,
        );
        break;
      }
    }
  });
}

#[derive(Default, Serialize, Deserialize)]
pub struct EmitTrigger {
  pub service: Option<String>,
  pub state: Option<String>,
  pub flow_type: Option<FlowType>,
  pub payload: Option<FlowPayload>,
  pub action: FlowAction,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceBranchContext {
  pub key: Option<String>,
  pub payload: Option<FlowPayload>,
  pub forced_user: Option<String>,
}

impl Into<serde_json::Value> for EmitTrigger {
  fn into(self) -> serde_json::Value {
    serde_json::to_value(self).unwrap_or_default()
  }
}

pub fn handle_ipc_start(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .cloned()
    .unwrap_or_default();

  let payload = msg
    .parse_payload::<ServicePayload>()
    .map_err(CoreError::Custom)?;

  let Some(uid) = msg.from_uid else {
    return Err(CoreError::PermissionDenied);
  };

  let svc = ctx
    .registry
    .metadata
    .find::<Service>("units", &payload.name);
  let caller = pm.users.lookup_by_uid(uid);
  let can_manage = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    true
  } else if let (Some(caller), Some(svc)) = (caller, svc.as_ref()) {
    match &svc.space {
      ServiceSpace::User => true,
      ServiceSpace::UserSelective { user } => user == &caller.username,
      ServiceSpace::System => false,
    }
  } else {
    false
  };

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  let only_user = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    None
  } else {
    caller.map(|u| u.username.clone())
  };

  let _ = dispatch.dispatch(
    "services",
    "start",
    serde_json::json!({ "name": payload.name, "only_user": only_user }).into(),
  );

  Ok(Message::ok(format!("started {}", payload.name)))
}

pub fn handle_ipc_stop(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .cloned()
    .unwrap_or_default();

  let payload = msg
    .parse_payload::<ServicePayload>()
    .map_err(CoreError::Custom)?;

  let Some(uid) = msg.from_uid else {
    return Err(CoreError::PermissionDenied);
  };

  let svc = ctx
    .registry
    .metadata
    .find::<Service>("units", &payload.name);
  let caller = pm.users.lookup_by_uid(uid);
  let can_manage = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    true
  } else if let (Some(caller), Some(svc)) = (caller, svc.as_ref()) {
    match &svc.space {
      ServiceSpace::User => true,
      ServiceSpace::UserSelective { user } => user == &caller.username,
      ServiceSpace::System => false,
    }
  } else {
    false
  };

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  let force = payload.force.unwrap_or(false);
  let only_user = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    None
  } else {
    caller.map(|u| u.username.clone())
  };
  let _ = dispatch.dispatch(
    "services",
    "stop",
    serde_json::json!({ "name": payload.name, "force": force, "only_user": only_user }).into(),
  );

  Ok(Message::ok(format!("stopped {}", payload.name)))
}

impl Runtime for ServiceRuntime {
  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<serde_json::Value>, CoreError> {
    match action {
      "watch_events" => {
        self.event_rx = Some(ctx.event_bus.subscribe::<rind_core::prelude::FlowEvent>());
      }
      "send_stdio" => {
        let endpoint = payload.get::<String>("endpoint")?;
        let message = payload
          .0
          .get("message")
          .cloned()
          .ok_or_else(|| CoreError::InvalidState("missing `message` for send_stdio".into()))
          .and_then(|v| {
            serde_json::from_value::<TransportMessage>(v)
              .map_err(|e| CoreError::InvalidState(format!("invalid send_stdio message: {e}")))
          })?;
        self.send_stdio_message(endpoint.as_str(), message);
      }
      "drain_events" => {
        if let Some(rx) = &self.event_rx {
          while let Some(w) = rx.try_recv() {
            self.broadcast_stdio_event(&w);
            let mut trig = EmitTrigger::default();
            trig.state = Some(w.name);
            trig.payload = Some(FlowPayload::from_json(Some(w.payload)));
            trig.flow_type = Some(match w.flow_type {
              rind_core::prelude::FlowEventType::State => FlowType::State,
              rind_core::prelude::FlowEventType::Signal => FlowType::Signal,
            });
            trig.action = w.action;
            let val: serde_json::Value = trig.into();
            let _ = dispatch.dispatch("services", "evaluate_triggers", val.into());
          }
        }

        while let Ok((service_name, message)) = self.stdio_rx.try_recv() {
          if message.name.as_deref() == Some("log") {
            let (level, message_text, fields) = self.stdio_log_entry(&service_name, &message);
            log.log(level, "service-transport", message_text, fields);
            continue;
          }
          let _ = dispatch.dispatch(
            "transport",
            "ingest",
            serde_json::json!({
              "endpoint": service_name,
              "message": message
            })
            .into(),
          );
        }
      }
      "evaluate_triggers" => {
        let emit_trig = payload.r#as::<EmitTrigger>().unwrap_or_default();

        // println!("{:?} {:?}", emit_trig.state, emit_trig.action);
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.to_string(), VariableHeap::KEY.to_string()),
            |registry, (sm, vh)| {
              let emit_event = match (
                emit_trig.state.as_ref(),
                emit_trig.flow_type,
                emit_trig.payload.as_ref(),
              ) {
                (Some(name), Some(flow_type), Some(payload)) => Some(FlowInstance {
                  name: name.clone(),
                  payload: payload.clone(),
                  r#type: flow_type,
                }),
                _ => None,
              };

              let services = registry
                .metadata
                .items::<Service>("units")
                .unwrap_or(Vec::new());

              for (unit, service) in services {
                let mut is_running = false;

                if let Some(instances) = registry
                  .instances
                  .get_mut(&format!("units@{}@{}", unit, service.name))
                {
                  for instance in instances.iter_mut() {
                    if let Some(service) = instance.downcast_mut::<Service>() {
                      is_running = service.instances.iter().any(|i| {
                        i.state == ServiceState::Active || i.state == ServiceState::Starting
                      });

                      if let Some(ref branching) = service.metadata.branching {
                        match (emit_trig.action, emit_event.as_ref(), is_running) {
                          (FlowAction::Revert, Some(event), true)
                            if branching.key.is_some()
                              && branching.enabled == true
                              && event.name == branching.source_state =>
                          {
                            let key = Self::branch_key_from_payload(
                              &event.payload,
                              branching.key.as_deref(),
                            );

                            let to_stop: Vec<String> = service
                              .instances
                              .iter()
                              .filter_map(|inst| {
                                if inst.state == ServiceState::Active
                                  && key.as_ref() == Some(&inst.key)
                                {
                                  return Some(inst.key.clone());
                                }
                                None
                              })
                              .collect();

                            for i in to_stop {
                              self.stop_service(
                                service,
                                StopMode::Graceful,
                                log,
                                dispatch,
                                Some(i),
                                None,
                              );
                            }
                          }
                          _ => {}
                        }
                      } else {
                        let should_stop = service
                          .metadata
                          .stop_on
                          .as_ref()
                          .map(|conds| {
                            conds.iter().any(|cond| {
                              crate::flow::condition_matches(sm, cond, emit_event.as_ref(), None)
                            })
                          })
                          .unwrap_or(false);

                        // TODO: Should it ignore stop_on?
                        let auto_stop_on_revert = if service.metadata.stop_on.is_none() {
                          match (
                            emit_trig.action,
                            emit_event.as_ref(),
                            service.metadata.start_on.as_ref(),
                          ) {
                            (FlowAction::Revert, Some(event), Some(start_conds)) => {
                              start_conds.iter().any(|cond| {
                                crate::triggers::check_condition(cond, event)
                                  && !crate::flow::condition_is_active(
                                    sm,
                                    cond,
                                    Some(&event.payload),
                                  )
                              })
                            }
                            _ => false,
                          }
                        } else {
                          false
                        };

                        if (should_stop || auto_stop_on_revert) && is_running {
                          self.stop_service(service, StopMode::Graceful, log, dispatch, None, None);
                        }
                      }
                    }
                  }
                }

                let should_start = service
                  .start_on
                  .as_ref()
                  .map(|conds| {
                    conds.iter().all(|cond| {
                      crate::flow::condition_matches(sm, cond, emit_event.as_ref(), None)
                    })
                  })
                  .unwrap_or(false);

                if !should_start {
                  continue;
                }

                if !service
                  .branching
                  .as_ref()
                  .map(|b| b.enabled)
                  .unwrap_or(false)
                  && is_running
                {
                  continue;
                }

                let ser =
                  registry.instantiate_one("units", &format!("{unit}@{}", service.name), |x| {
                    Ok(Service::new(x))
                  })?;

                if let Some(branching) = &ser.metadata.branching {
                  if branching.enabled {
                    let branches = sm
                      .states
                      .get(&branching.source_state)
                      .cloned()
                      .unwrap_or_default();

                    let mut started = 0usize;
                    for branch in branches {
                      let Some(key) =
                        Self::branch_key_from_payload(&branch.payload, branching.key.as_deref())
                      else {
                        continue;
                      };

                      if ser.instances.iter().any(|i| i.key == key) {
                        continue;
                      }
                      if let Some(max) = branching.max_instances {
                        if ser.instances.len() >= max || started >= max {
                          break;
                        }
                      }
                      let branch_ctx = ServiceBranchContext {
                        key: Some(key.clone()),
                        payload: Some(branch.payload.clone()),
                        forced_user: None,
                      };
                      match self.spawn_all(ser, log, Some(&branch_ctx), Some(sm), Some(vh)) {
                        Ok(instances) => {
                          ser.instances.extend(instances);
                          self.register_stdio_transport(ser, dispatch);
                          let _ = dispatch.dispatch(
                          "transport",
                          "register_stdio",
                          serde_json::json!({ "endpoint": format!("{unit}@{}", ser.metadata.name) })
                            .into(),
                        );
                          started += 1;
                        }
                        Err(e) => {
                          let mut fields = self.log_fields(ser, "start");
                          fields.insert("branch".into(), key);
                          fields.insert("error".into(), e.to_string());
                          log.log(
                            LogLevel::Error,
                            "service-runtime",
                            "failed to start branched service instance",
                            fields,
                          );
                        }
                      }
                    }
                    continue;
                  }
                }

                self.start_service(ser, log, Some(sm), dispatch, Some(vh));
              }
              Ok(())
            },
          )?;
      }
      "start" => {
        let name = payload.get::<String>("name")?;
        let only_user = payload.get::<String>("only_user").ok();
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.to_string(), VariableHeap::KEY.to_string()),
            |registry, (sm, vh)| {
              let service =
                registry.instantiate_one::<Service>("units", &name, |x| Ok(Service::new(x)))?;
              if let Some(user) = only_user.clone() {
                let launch_ctx = ServiceBranchContext {
                  key: None,
                  payload: None,
                  forced_user: Some(user),
                };

                match self.spawn_all(service, log, Some(&launch_ctx), Some(sm), Some(vh)) {
                  Ok(instances) => {
                    service.instances.extend(instances);
                    self.register_stdio_transport(service, dispatch);
                    if let Some(inst) = service.instances.as_one_mut() {
                      inst.state = ServiceState::Active;
                      self.run_triggers(service.metadata.on_start.as_ref(), dispatch);
                    }
                    let _ = dispatch.dispatch(
                      "services",
                      "reconcile_stacks",
                      json!({
                        "service": service.metadata.name,
                        "id": service.id.0,
                        "action": ServiceEventKind::Started
                      })
                      .into(),
                    );
                  }
                  Err(e) => {
                    let err = format!("Failed to start service \"{}\": {e}", service.metadata.name);
                    let mut fields = self.log_fields(service, "start");
                    fields.insert("error".into(), e.to_string());
                    log.log(
                      LogLevel::Error,
                      "service-runtime",
                      "failed to start service",
                      fields,
                    );
                    if let Some(inst) = service.instances.as_one_mut() {
                      inst.state = ServiceState::Error(err);
                    }
                  }
                }
              } else {
                self.start_service(service, log, Some(sm), dispatch, Some(vh));
              }
              if service
                .metadata
                .transport
                .as_ref()
                .map(is_stdio_transport)
                .unwrap_or(false)
              {
                let _ = dispatch.dispatch(
                  "transport",
                  "register_stdio",
                  serde_json::json!({ "endpoint": name }).into(),
                );
              }
              Ok(())
            },
          )?;
      }
      "stop" => {
        let name = payload.get::<String>("name")?;
        let force = payload.get::<bool>("force").unwrap_or(false);
        let mode = if force {
          StopMode::ForceKill
        } else {
          StopMode::Graceful
        };

        let service = ctx
          .registry
          .instantiate_one::<Service>("units", &name, |x| Ok(Service::new(x)))?;
        let only_user = payload.get::<String>("only_user").ok();
        self.stop_service(service, mode, log, dispatch, None, only_user);
      }
      "stop_all" => {
        let force = payload.get::<bool>("force").unwrap_or(false);
        let mode = if force {
          StopMode::ForceKill
        } else {
          StopMode::Graceful
        };

        let keys: Vec<String> = ctx
          .registry
          .instances
          .keys()
          .filter(|k| k.starts_with("units@"))
          .cloned()
          .collect();

        for key in keys {
          if let Some(instances) = ctx.registry.instances.get_mut(&key) {
            for instance in instances.iter_mut() {
              if let Some(service) = instance.downcast_mut::<Service>() {
                self.stop_service(service, mode, log, dispatch, None, None);
              }
            }
          }
        }
      }
      "start_all" => {
        let metadata = ctx
          .registry
          .metadata
          .metadata("units")
          .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;

        let mut all_services: Vec<(String, Arc<ServiceMetadata>)> = Vec::new();
        for group in metadata.groups() {
          if let Some(svcs) = ctx.registry.metadata.group_items::<Service>("units", group) {
            for svc in svcs {
              all_services.push((format!("{group}@{}", svc.name), svc));
            }
          }
        }

        let mut started: HashSet<String> = HashSet::new();
        let mut pending: Vec<(String, Vec<String>, Arc<ServiceMetadata>)> = Vec::new();
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.to_string(), VariableHeap::KEY.to_string()),
            |registry, (sm, vh)| {
              for (full_name, svc_meta) in &all_services {
                if let Some(afters) = &svc_meta.after {
                  pending.push((full_name.clone(), afters.clone(), svc_meta.clone()));
                } else {
                  let service = registry
                    .instantiate_one::<Service>("units", &full_name, |x| Ok(Service::new(x)))?;
                  self.start_service(service, log, Some(sm), dispatch, Some(vh));
                  started.insert(full_name.clone());
                }
              }

              loop {
                let mut progress = false;
                pending.retain(|(name, afters, _meta)| {
                  if afters.iter().all(|a| started.contains(a)) {
                    if let Ok(service) =
                      registry.instantiate_one::<Service>("units", name, |x| Ok(Service::new(x)))
                    {
                      self.start_service(service, log, Some(sm), dispatch, Some(vh));
                      started.insert(name.clone());
                      progress = true;
                    }
                    false
                  } else {
                    true
                  }
                });
                if !progress {
                  break;
                }
              }
              Ok(())
            },
          )?;

        if !pending.is_empty() {
          let mut fields = HashMap::new();
          let names: Vec<String> = pending.iter().map(|(n, _, _)| n.clone()).collect();
          fields.insert("unresolved".to_string(), names.join(", "));
          log.log(
            LogLevel::Error,
            "service-runtime",
            "unresolved service dependencies",
            fields,
          );
        }
      }
      "reconcile_stacks" => {
        let service = payload.get::<String>("service")?;
        let action = payload.get::<ServiceEventKind>("action")?;

        let metadata = ctx
          .registry
          .metadata
          .metadata("units")
          .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;

        let mut dependents: Vec<(String, Arc<ServiceMetadata>)> = Vec::new();
        for group in metadata.groups() {
          if let Some(svcs) = ctx.registry.metadata.group_items::<Service>("units", group) {
            for svc in svcs {
              if let Some(ref dependencies) = svc.after
                && dependencies.contains(&service)
              {
                dependents.push((format!("{group}@{}", svc.name), svc));
              }
            }
          }
        }

        // these minefields that i walk through
        // oooh, what i'd risk to be close to you
        // ooooooooh, these minefields, keeeping me from you
        // woooaaah what i'd risk to be close to you
        // close to youuuuuuuuu ooooh
        match action {
          ServiceEventKind::Failed
          | ServiceEventKind::Stopped
          | ServiceEventKind::Exited { code: _ } => {
            for (dependent, _) in dependents {
              if let Ok(service) = ctx.registry.as_one_mut::<Service>("units", &dependent) {
                self.stop_service(service, StopMode::Graceful, log, dispatch, None, None);
              }
            }
          }
          ServiceEventKind::Started => {
            ctx
              .registry
              .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
                (StateMachine::KEY.to_string(), VariableHeap::KEY.to_string()),
                |registry, (sm, vh)| {
                  for (dependent, svc) in dependents {
                    let should_start = svc.after.as_ref().unwrap().iter().any(|a| {
                      if let Ok(ref svc) = registry.as_one::<Service>("units", a) {
                        !svc.instances.is_empty()
                          && !svc.instances.iter().any(|x| {
                            x.state == ServiceState::Inactive
                              || x.state == ServiceState::Stopping
                              || matches!(x.state, ServiceState::Exited(_))
                              || matches!(x.state, ServiceState::Error(_))
                          })
                      } else {
                        false
                      }
                    });

                    if should_start {
                      let service =
                        registry.instantiate_one::<Service>("units", &dependent, |x| {
                          Ok(Service::new(x))
                        })?;
                      self.start_service(service, log, Some(sm), dispatch, Some(vh));
                    }
                  }
                  Ok(())
                },
              )?;
          }
        }
      }
      "child_exited" => {
        let pid = payload.get::<i64>("pid")? as i32;
        let code = payload.get::<i64>("code")? as i32;

        // TODO: Move to newer instancing impl
        let keys: Vec<String> = ctx
          .registry
          .instances
          .keys()
          .filter(|k| k.starts_with("units@"))
          .cloned()
          .collect();

        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.to_string(), VariableHeap::KEY.to_string()),
            |registry, (sm, vh)| {
              for key in keys {
                if let Some(instances) = registry.instances.get_mut(&key) {
                  for instance in instances.iter_mut() {
                    if let Some(service) = instance.downcast_mut::<Service>() {
                      if let Some(exit_action) =
                        self.handle_child_exit(service, pid, code, log, dispatch)
                      {
                        match exit_action {
                          ServiceExitAction::Restart => {
                            self.start_service(service, log, Some(sm), dispatch, Some(vh));
                          }
                          ServiceExitAction::StopDependents => {
                            let _ = dispatch.dispatch(
                              "services",
                              "reconcile_stacks",
                              json!({
                                "service": service.metadata.name,
                                "id": service.id.0,
                                "action": ServiceEventKind::Exited { code }
                              })
                              .into(),
                            );
                          }
                        }
                      }
                    }
                  }
                }
              }
              Ok(())
            },
          )?;
      }
      "timeout_sweep" => {
        // TODO: Move to newer instancing impl
        let keys: Vec<String> = ctx
          .registry
          .instances
          .keys()
          .filter(|k| k.starts_with("units@"))
          .cloned()
          .collect();

        for key in keys {
          if let Some(instances) = ctx.registry.instances.get_mut(&key) {
            for instance in instances.iter_mut() {
              if let Some(service) = instance.downcast_mut::<Service>() {
                self.timeout_sweep(service);
              }
            }
          }
        }
      }
      _ => {}
    }
    Ok(None)
  }

  fn id(&self) -> &str {
    "services"
  }
}
