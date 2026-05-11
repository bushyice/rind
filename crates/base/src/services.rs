use nix::sys::signal::{Signal, kill};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use nix::unistd::Pid;
use rind_ipc::Message;
use rind_ipc::payloads::SSPayload;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::ops::{Deref, DerefMut};
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use rind_core::{notifier::Notifier, prelude::*};

use crate::flow::{
  FlowInstance, FlowItem, FlowPayload, FlowRuntimePayload, FlowType, StateMachine, Trigger,
};
use crate::permissions::PERM_SYSTEM_SERVICES;
use crate::prelude::trigger_events;
use crate::sockets::get_all_sockets;
use crate::transport::{TransportMessage, TransportMethod, start_stdout_listener};
use crate::variables::VariableHeap;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RunOption {
  #[serde(default)]
  pub exec: Ustr,
  #[serde(default)]
  pub args: Vec<Ustr>,
  pub env: Option<HashMap<String, String>>,
  pub variable: Option<String>,
}

impl RunOption {
  pub fn to_string(&self) -> String {
    format!(
      "{} {}",
      self.exec,
      self
        .args
        .iter()
        .map(|a| a.as_str())
        .collect::<Vec<_>>()
        .join(" ")
    )
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
  pub key: Ustr,
  pub user: Option<Ustr>,
  pub child: Option<Child>,
  pub state: ServiceState,
  pub retry_count: u32,
  pub stop_time: Option<Instant>,
  pub manually_stopped: bool,
}

impl ChildInstance {
  pub fn new(key: impl Into<Ustr>, user: Option<Ustr>, child: Option<Child>) -> Self {
    Self {
      key: key.into(),
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
  #[serde(rename = "source-state")]
  pub source_state: Ustr,
  #[serde(default)]
  pub key: Option<String>,
  #[serde(rename = "max-instances", default)]
  pub max_instances: Option<usize>,
  #[serde(default)]
  pub only: Option<Vec<String>>,
  #[serde(default)]
  pub except: Option<Vec<String>>,
}

fn default_username_field() -> String {
  "username".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceUserSource {
  pub state: Ustr,
  #[serde(rename = "username-field", default = "default_username_field")]
  pub username_field: String,
  #[serde(rename = "match-branch-key")]
  pub match_branch_key: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceCgroup {
  pub path: Option<Ustr>,
  #[serde(rename = "memory-max")]
  pub memory_max: Option<Ustr>,
  #[serde(rename = "cpu-max")]
  pub cpu_max: Option<Ustr>,
  #[serde(rename = "pids-max")]
  pub pids_max: Option<Ustr>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ServiceNamespaces {
  #[serde(default)]
  pub mount: bool,
  #[serde(default)]
  pub uts: bool,
  #[serde(default)]
  pub ipc: bool,
  #[serde(default)]
  pub net: bool,
  #[serde(default)]
  pub pid: bool,
  #[serde(default)]
  pub user: bool,
  #[serde(default)]
  pub cgroup: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WatchdogAction {
  #[default]
  Restart,
  Stop,
  Signal,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceWatchdog {
  #[serde(rename = "interval-ms")]
  pub interval_ms: Option<u64>,
  #[serde(rename = "grace-ms", default = "default_watchdog_grace_ms")]
  pub grace_ms: u64,
  #[serde(default)]
  pub action: WatchdogAction,
}

fn default_watchdog_grace_ms() -> u64 {
  15_000
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceSpace {
  #[default]
  System,
  User,
  UserSelective {
    user: Ustr,
  },
}

#[model(
  meta_name = name,
  meta_fields(
    name, run, after, branching, restart, start_on, stop_on, on_start, on_stop,
    transport, working_dir, space, user_source, singleton, managed_by, cgroup,
    namespaces, watchdog
  ),
  derive_metadata(Debug)
)]
pub struct Service {
  // Metadata
  pub name: Ustr,
  // pub run: Vec<RunOption>,
  pub run: RunOption,
  pub after: Option<Vec<Ustr>>,
  #[serde(rename = "start-on")]
  pub start_on: Option<Vec<FlowItem>>,
  #[serde(rename = "stop-on")]
  pub stop_on: Option<Vec<FlowItem>>,
  #[serde(rename = "on-start")]
  pub on_start: Option<Vec<Trigger>>,
  #[serde(rename = "on-stop")]
  pub on_stop: Option<Vec<Trigger>>,
  #[serde(rename = "working-dir")]
  pub working_dir: Option<Ustr>,
  #[serde(default, rename = "space")]
  pub space: ServiceSpace,
  #[serde(default)]
  pub singleton: bool,
  #[serde(rename = "user-source")]
  pub user_source: Option<ServiceUserSource>,
  pub transport: Option<TransportMethod>,
  pub branching: Option<BranchingConfig>,
  pub restart: Option<RestartPolicy>,
  #[serde(rename = "managed-by")]
  pub managed_by: Option<Vec<Ustr>>,
  pub cgroup: Option<ServiceCgroup>,
  pub namespaces: Option<ServiceNamespaces>,
  pub watchdog: Option<ServiceWatchdog>,

  // Instance data
  pub id: ServiceId,
  pub instances: ChildInstanceGroup,
  pub last_state: ServiceState,
}

impl Service {
  pub fn new(metadata: Arc<ServiceMetadata>) -> Self {
    Self {
      metadata,
      id: ServiceId::default(),
      instances: ChildInstanceGroup::default(),
      last_state: ServiceState::Inactive,
    }
  }
}

pub struct ServiceRuntime {
  event_rx: Option<rind_core::events::Subscription<rind_core::prelude::FlowEvent>>,
  stdio_tx: Sender<(Ustr, TransportMessage)>,
  stdio_rx: Receiver<(Ustr, TransportMessage)>,
  stdio_writers: Mutex<HashMap<Ustr, Vec<Sender<TransportMessage>>>>,
  pid_map: HashMap<u32, Ustr>,
  stopping_map: HashMap<u32, Instant>,
  trigger_index: HashMap<Ustr, HashSet<Ustr>>,
  watchdog_fds: HashMap<RawFd, WatchdogBinding>,
  watchdog_pids: HashMap<u32, RawFd>,
}

#[derive(Debug, Clone)]
struct WatchdogBinding {
  service_key: Ustr,
  branch: Option<Ustr>,
  user: Option<Ustr>,
  pid: u32,
}

impl Default for ServiceRuntime {
  fn default() -> Self {
    let (stdio_tx, stdio_rx) = mpsc::channel();
    Self {
      event_rx: None,
      stdio_tx,
      stdio_rx,
      stdio_writers: Mutex::new(HashMap::new()),
      pid_map: HashMap::new(),
      stopping_map: HashMap::new(),
      trigger_index: HashMap::new(),
      watchdog_fds: HashMap::new(),
      watchdog_pids: HashMap::new(),
    }
  }
}

impl ServiceRuntime {
  fn sanitize_cgroup_component(input: &str) -> String {
    input
      .chars()
      .map(|c| {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
          c
        } else {
          '_'
        }
      })
      .collect()
  }

  fn cgroup_path_for(
    service: &Service,
    branch_ctx: Option<&ServiceBranchContext>,
    user: Option<&Ustr>,
  ) -> Option<PathBuf> {
    let Some(cg) = &service.metadata.cgroup else {
      return None;
    };

    let mut path = if let Some(custom) = &cg.path {
      PathBuf::from(custom.as_str())
    } else {
      let mut p = PathBuf::from("/sys/fs/cgroup/rind");
      p.push(Self::sanitize_cgroup_component(
        service.metadata.name.as_str(),
      ));
      if let Some(branch) = branch_ctx.and_then(|ctx| ctx.key.as_ref()) {
        p.push(Self::sanitize_cgroup_component(branch.as_str()));
      }
      if let Some(user) = user {
        p.push(Self::sanitize_cgroup_component(user.as_str()));
      }
      p
    };

    if !path.is_absolute() {
      path = PathBuf::from("/sys/fs/cgroup").join(path);
    }

    Some(path)
  }

  fn setup_cgroup_for_pid(
    &self,
    service: &Service,
    branch_ctx: Option<&ServiceBranchContext>,
    user: Option<&Ustr>,
    pid: u32,
  ) -> anyhow::Result<()> {
    let Some(path) = Self::cgroup_path_for(service, branch_ctx, user) else {
      return Ok(());
    };

    std::fs::create_dir_all(&path)?;

    if let Some(cg) = &service.metadata.cgroup {
      if let Some(mem) = &cg.memory_max {
        let _ = std::fs::write(path.join("memory.max"), mem.as_str());
      }
      if let Some(cpu) = &cg.cpu_max {
        let _ = std::fs::write(path.join("cpu.max"), cpu.as_str());
      }
      if let Some(pids) = &cg.pids_max {
        let _ = std::fs::write(path.join("pids.max"), pids.as_str());
      }
    }

    std::fs::write(path.join("cgroup.procs"), pid.to_string())?;
    Ok(())
  }

  fn namespace_unshare_flags(ns: &ServiceNamespaces) -> libc::c_int {
    let mut flags = 0;
    if ns.mount {
      flags |= libc::CLONE_NEWNS;
    }
    if ns.uts {
      flags |= libc::CLONE_NEWUTS;
    }
    if ns.ipc {
      flags |= libc::CLONE_NEWIPC;
    }
    if ns.net {
      flags |= libc::CLONE_NEWNET;
    }
    if ns.cgroup {
      flags |= libc::CLONE_NEWCGROUP;
    }
    flags
  }

  fn arm_watchdog_timer(
    &mut self,
    service_key: Ustr,
    branch: Option<Ustr>,
    user: Option<Ustr>,
    pid: u32,
    watchdog: &ServiceWatchdog,
    resources: &mut Resources,
  ) -> Result<(), CoreError> {
    let tfd = TimerFd::new(
      ClockId::CLOCK_MONOTONIC,
      TimerFlags::TFD_NONBLOCK | TimerFlags::TFD_CLOEXEC,
    )
    .map_err(CoreError::custom)?;

    let grace = Duration::from_millis(watchdog.grace_ms.max(1));
    tfd
      .set(
        Expiration::OneShot(TimeSpec::from(grace)),
        TimerSetTimeFlags::empty(),
      )
      .map_err(CoreError::custom)?;

    let fd = tfd.as_fd().as_raw_fd();
    resources.own(fd, tfd);
    let payload_service_key = service_key.clone();
    let payload_branch = branch.clone();
    resources.action(
      fd,
      ResourceAction::from(("services", "watchdog_expired")).payload(move |p| {
        let p = p.insert("service_key", payload_service_key.clone());
        if let Some(branch) = &payload_branch {
          p.insert("branch", branch.clone())
        } else {
          p
        }
      }),
    );

    self.watchdog_fds.insert(
      fd,
      WatchdogBinding {
        service_key,
        branch,
        user,
        pid,
      },
    );
    self.watchdog_pids.insert(pid, fd);
    Ok(())
  }

  fn disarm_watchdog_pid(&mut self, pid: u32, resources: &mut Resources) {
    if let Some(fd) = self.watchdog_pids.remove(&pid) {
      self.watchdog_fds.remove(&fd);
      resources.terminate(fd);
    }
  }

  fn refresh_watchdog_fd(&self, fd: RawFd, watchdog: &ServiceWatchdog) -> Result<(), CoreError> {
    let grace = Duration::from_millis(watchdog.grace_ms.max(1));
    let spec = libc::itimerspec {
      it_interval: libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
      },
      it_value: libc::timespec {
        tv_sec: grace.as_secs() as libc::time_t,
        tv_nsec: grace.subsec_nanos() as libc::c_long,
      },
    };
    let rc = unsafe { libc::timerfd_settime(fd, 0, &spec, std::ptr::null_mut()) };
    if rc < 0 {
      return Err(CoreError::custom(std::io::Error::last_os_error()));
    }
    Ok(())
  }

  fn payload_field_as_key(payload: &FlowPayload, field: &str) -> Option<Ustr> {
    payload.get_json_field(field).map(|v| {
      if let Some(s) = v.as_str() {
        Ustr::from(s)
      } else {
        Ustr::from(v.to_string())
      }
    })
  }

  fn branch_key_from_payload(payload: &FlowPayload, key_name: Option<&str>) -> Option<Ustr> {
    if let Some(key_name) = key_name {
      return Self::payload_field_as_key(payload, key_name).filter(|v| !v.as_str().is_empty());
    }
    let value = payload.to_string_payload();
    if value.is_empty() {
      None
    } else {
      Some(Ustr::from(value))
    }
  }

  fn resolve_user_from_source(
    &self,
    source: &ServiceUserSource,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&StateMachine>,
  ) -> anyhow::Result<Option<Ustr>> {
    let Some(sm) = sm else {
      return Ok(None);
    };
    let Some(branches) = sm.states.get(&source.state) else {
      return Ok(None);
    };

    if let Some(field) = &source.match_branch_key {
      let Some(expected) = branch_ctx.and_then(|ctx| ctx.key.as_ref()) else {
        return Ok(None);
      };
      let mut matches = HashSet::new();
      for branch in branches {
        let Some(found) = Self::payload_field_as_key(&branch.payload, field) else {
          continue;
        };
        if found != *expected {
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
  ) -> anyhow::Result<Option<Ustr>> {
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
          && let Some(sessions) = sm.states.get("rind:user_session")
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

  fn rebuild_trigger_index(&mut self, metadata: &MetadataRegistry) {
    self.trigger_index.clear();
    let services = metadata.items::<Service>("units").unwrap_or_default();

    for (group, meta) in services {
      let key = Ustr::from(format!("{}:{}", group, meta.name));

      let mut interests = HashSet::new();
      if let Some(start_on) = &meta.start_on {
        for item in start_on {
          interests.insert(item.name());
        }
      }
      if let Some(stop_on) = &meta.stop_on {
        for item in stop_on {
          interests.insert(item.name());
        }
      }

      for interest in interests {
        self
          .trigger_index
          .entry(interest.clone())
          .or_default()
          .insert(key.clone());
      }
    }
  }

  pub fn spawn_all(
    &mut self,
    service: &Service,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    branch_ctx: Option<&ServiceBranchContext>,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&StateMachine>,
    variable_heap: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
  ) -> anyhow::Result<Vec<ChildInstance>> {
    let mut instances = Vec::new();

    if let Some(sm) = sm {
      let key = registry_key.trim_start_matches("units:");

      if let Some(inst) = sm.states.get("rind:inactive")
        && inst.iter().any(|x| x.payload.to_string_payload() == key)
      {
        return Ok(instances);
      }
    }

    let resolved = self.resolve_run_option(&service.metadata.run, variable_heap);
    let run_ref = resolved.as_ref().unwrap_or(&service.metadata.run);
    let instance = self.spawn_process(
      service,
      run_ref,
      log,
      dispatch,
      branch_ctx,
      sockets_map,
      sm,
      variable_heap,
      registry_key.clone(),
      notifier.clone(),
      resources,
    )?;

    instances.push(instance);

    Ok(instances)
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
      .unwrap_or_default();
    let args = table
      .get("args")
      .and_then(|v| v.as_array())
      .map(|arr| {
        arr
          .iter()
          .filter_map(|v| v.as_str().map(|s| Ustr::from(s)))
          .collect()
      })
      .unwrap_or_default();
    let env = table.get("env").and_then(|v| v.as_table()).map(|t| {
      t.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
    });

    Some(RunOption {
      exec: Ustr::from(exec),
      args,
      env,
      variable: None,
    })
  }

  pub fn spawn_service(
    &mut self,
    service: &mut Service,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&StateMachine>,
    variable_heap: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
  ) -> anyhow::Result<()> {
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service started",
      self.log_fields(service, "start"),
    );

    let instances = self.spawn_all(
      service,
      log,
      dispatch,
      None,
      sockets_map,
      sm,
      variable_heap,
      registry_key,
      notifier,
      resources,
    )?;
    service.instances.extend(instances);
    Ok(())
  }

  fn log_fields(&self, service: &Service, action: impl Into<Ustr>) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    fields.insert("action".to_string(), action.into().to_string());
    fields.insert("service".to_string(), service.metadata.name.to_string());
    fields
  }

  fn spawn_process(
    &mut self,
    service: &Service,
    run: &RunOption,
    log: &LogHandle,
    _dispatch: &RuntimeDispatcher,
    branch_ctx: Option<&ServiceBranchContext>,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&StateMachine>,
    variables: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
  ) -> anyhow::Result<ChildInstance> {
    let full_name = registry_key
      .strip_prefix("units:")
      .map(|n| n.to_ustr())
      .unwrap();
    let mut args = run.args.clone();
    let mut envs = run.env.clone().unwrap_or_default();
    let branch_key = branch_ctx.and_then(|ctx| ctx.key.as_ref());
    let resolved_user = self.resolve_service_user(service, branch_ctx, sm)?;
    let watchdog_cfg = service.metadata.watchdog.clone();
    if let Some(ns) = &service.metadata.namespaces {
      if ns.pid {
        return Err(anyhow::anyhow!(
          "service '{}' requested pid namespace, but pid isolation is unsupported with current pre_exec+unshare model (requires clone/fork+exec redesign)",
          service.metadata.name
        ));
      }
      if ns.user {
        return Err(anyhow::anyhow!(
          "service '{}' requested user namespace, but user namespace setup is unsupported without uid/gid map configuration",
          service.metadata.name
        ));
      }
    }

    if let Some(transport) = &service.metadata.transport {
      if let Some(sm) = sm {
        let resolve_state = |spec: &str| -> Option<String> {
          let (state_name, path) = spec
            .split_once('/')
            .map(|(name, p)| (name, Some(p)))
            .unwrap_or((spec, None));
          // TODO: use BranchCtx only if the name of state matches
          let payload = if let Some(payload) = branch_ctx.and_then(|x| x.payload.as_ref())
            && state_name == "$"
          {
            payload
          } else {
            sm.states
              .get(state_name)
              .and_then(|v| v.first())
              .map(|x| &x.payload)?
          };
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
            FlowPayload::String(s) => Some(s.clone()),
            FlowPayload::Bytes(b) => Some(String::from_utf8(b.clone()).unwrap_or_default()),
            FlowPayload::None(_) => Some(String::new()),
          }
        };

        match transport {
          crate::transport::TransportMethod::Options {
            id,
            options,
            permissions: _,
          } if id.0.as_str() == "env" => {
            for option in options.iter() {
              let Some((key, value)) = option.split_once('=') else {
                continue;
              };
              if let Some(state_name) = value.strip_prefix("state:") {
                if let Some(val) = resolve_state(state_name) {
                  envs.insert(key.to_string(), val);
                }
              } else if let (Some(variables), Some(variable)) =
                (variables, value.strip_prefix("var:"))
              {
                if let Some(val) = variables.get(variable) {
                  envs.insert(key.to_string(), val.to_string());
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
          } if id.0.as_str() == "args" => {
            for option in options.iter() {
              if let Some(state_name) = option.strip_prefix("state:") {
                let payload = resolve_state(state_name).unwrap_or_default();
                if !payload.is_empty() {
                  args.push(payload.into());
                }
              } else if let (Some(variables), Some(variable)) =
                (variables, option.strip_prefix("var:"))
              {
                if let Some(val) = variables.get(variable) {
                  args.push(Ustr::from(val.to_string()));
                }
              } else {
                args.push(option.to_ustr());
              }
            }
          }
          _ => {}
        }
      }
    }

    let mut activation_fds = Vec::new();
    let mut activation_names = Vec::new();

    if let Some(activation) = sockets_map.get(&full_name) {
      activation_fds.extend(activation.fds.clone());
      activation_names.extend(activation.names.clone());
    }

    if !activation_fds.is_empty() {
      let inherited_fds = (0..activation_fds.len())
        .map(|i| (3 + i).to_string())
        .collect::<Vec<_>>()
        .join(",");
      envs.insert("RIND_SOCKET_FDS".to_string(), inherited_fds);
      envs.insert(
        "RIND_SOCKET_COUNT".to_string(),
        activation_fds.len().to_string(),
      );
      envs.insert("LISTEN_FDS".to_string(), activation_fds.len().to_string());
      if !activation_names.is_empty() {
        let names = activation_names
          .iter()
          .map(|x| x.as_str())
          .collect::<Vec<_>>()
          .join(":");
        envs.insert("RIND_SOCKET_NAMES".to_string(), names.clone());
        envs.insert("LISTEN_FDNAMES".to_string(), names);
      }
    }

    if let Some(watchdog) = &watchdog_cfg {
      envs.insert(
        "RIND_WATCHDOG_GRACE_MS".to_string(),
        watchdog.grace_ms.to_string(),
      );
      if let Some(interval_ms) = watchdog.interval_ms {
        envs.insert(
          "RIND_WATCHDOG_INTERVAL_MS".to_string(),
          interval_ms.to_string(),
        );
      }
    }

    let child = unsafe {
      let mut cmd = Command::new(run.exec.as_str());
      let pre_exec_fds = activation_fds.clone();
      let ns_flags = service
        .metadata
        .namespaces
        .as_ref()
        .map(Self::namespace_unshare_flags)
        .unwrap_or(0);
      cmd
        .args(args.iter().map(|a| a.as_str()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .pre_exec(move || {
          libc::setsid();
          if ns_flags != 0 && libc::unshare(ns_flags) < 0 {
            return Err(std::io::Error::last_os_error());
          }
          for (idx, fd) in pre_exec_fds.iter().enumerate() {
            let target_fd = (3 + idx) as RawFd;
            if *fd != target_fd && libc::dup2(*fd, target_fd) < 0 {
              return Err(std::io::Error::last_os_error());
            }
            let flags = libc::fcntl(target_fd, libc::F_GETFD);
            if flags >= 0 {
              let _ = libc::fcntl(target_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
          }
          Ok(())
        });
      let user_info = if let Some(username) = resolved_user.as_ref() {
        let store = rind_core::user::UserStore::load_system()
          .map_err(|e| anyhow::anyhow!("failed to load user store: {e}"))?;
        let Some(user) = store.lookup_by_name(username.as_str()) else {
          return Err(anyhow::anyhow!(
            "user '{}' not found for service '{}'",
            username,
            service.metadata.name
          ));
        };
        Some((user.uid, user.gid, user.home.clone(), username.clone()))
      } else {
        None
      };

      if let Some(dir) = &service.metadata.working_dir {
        cmd.current_dir(dir.as_str());
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
          && dir.as_str().starts_with("~")
        {
          cmd.current_dir(format!("{}{}", home, &dir.as_str()[1..]));
        }

        envs.extend(read_env_file(&format!("{home}/.env")));

        envs.insert("HOME".into(), home);
        envs.insert("USER".into(), username.to_string());
      }

      if let Some(key) = branch_key {
        cmd.env("RIND_BRANCH_KEY", key.as_str());
      }
      if !envs.is_empty() {
        cmd.envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
      }
      cmd.spawn()?
    };

    let pid = child.id();
    if let Err(e) = self.setup_cgroup_for_pid(service, branch_ctx, resolved_user.as_ref(), pid) {
      let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
      return Err(anyhow::anyhow!(
        "failed to setup cgroup for service '{}': {e}",
        service.metadata.name
      ));
    }
    self.pid_map.insert(pid, registry_key.clone());

    let mut child = child;
    if service
      .metadata
      .transport
      .as_ref()
      .map(is_stdio_transport)
      .unwrap_or(false)
    {
      start_stdout_listener(
        registry_key.clone(),
        &mut child,
        self.stdio_tx.clone(),
        notifier,
      );
      let (tx, rx) = mpsc::channel::<TransportMessage>();
      start_stdin_writer(registry_key.clone(), &mut child, log.clone(), rx);
      if let Ok(mut writers) = self.stdio_writers.lock() {
        writers.entry(registry_key.clone()).or_default().push(tx);
      }
    } else {
      start_service_stream_logs(registry_key.clone(), &mut child, log.clone());
    }

    if let Some(watchdog) = &watchdog_cfg {
      let _ = self.arm_watchdog_timer(
        registry_key.clone(),
        branch_key.cloned(),
        resolved_user.clone(),
        pid,
        watchdog,
        resources,
      );
    }

    Ok(ChildInstance::new(
      branch_key.cloned().unwrap_or_default(),
      resolved_user,
      Some(child),
    ))
  }

  pub fn start_service(
    &mut self,
    service: &mut Service,
    log: &LogHandle,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&StateMachine>,
    dispatch: &RuntimeDispatcher,
    variable_heap: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
  ) {
    if let Some(inst) = service.instances.as_one_mut() {
      if inst.state == ServiceState::Active
        || inst.state == ServiceState::Starting
        || inst.state == ServiceState::Stopping
      {
        if service.metadata.singleton {
          return;
        }
      }
    }

    match self.spawn_service(
      service,
      log,
      dispatch,
      sockets_map,
      sm,
      variable_heap,
      registry_key.clone(),
      notifier,
      resources,
    ) {
      Ok(_) => {
        self.register_stdio_transport(service, dispatch, None);
        if let Some(inst) = service.instances.as_one_mut() {
          inst.state = ServiceState::Active;
          self.run_triggers(service.metadata.on_start.as_ref(), sm, dispatch);
        }

        let _ = dispatch.dispatch(
          "services",
          "reconcile_stacks",
          rpayload!({
            "service": registry_key.clone(),
            "id": service.id.0,
            "action": ServiceEventKind::Started
          }),
        );

        let _ = dispatch.dispatch(
          "timer",
          "reconcile_timers",
          rpayload!({
            "service": registry_key.clone(),
            "id": service.id.0,
            "action": ServiceEventKind::Started
          }),
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

  fn stop_service_instance(
    &mut self,
    inst: &mut ChildInstance,
    service: Arc<ServiceMetadata>,
    mode: StopMode,
    dispatch: &RuntimeDispatcher,
    sm: Option<&StateMachine>,
    key: Option<Ustr>,
    user: Option<Ustr>,
    resources: &mut Resources,
  ) {
    if let Some(ref key) = key {
      if inst.key.as_str() != key.as_str() {
        return;
      }
    };
    if let Some(ref user) = user {
      let matches_owner = inst
        .user
        .as_ref()
        .map(|u| u.as_str() == user.as_str())
        .unwrap_or(false)
        || (inst.user.is_none() && inst.key.as_str() == user.as_str());
      if !matches_owner {
        return;
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
      self.disarm_watchdog_pid(child.id(), resources);
      inst.state = ServiceState::Stopping;
      inst.stop_time = Some(Instant::now());
      inst.manually_stopped = true;
      self.stopping_map.insert(child.id(), Instant::now());
    } else {
      if inst.state == ServiceState::Active {
        self.run_triggers(service.on_stop.as_ref(), sm, dispatch);
      }
      inst.state = ServiceState::Inactive;
    }
  }

  pub fn stop_service(
    &mut self,
    service: &mut Service,
    mode: StopMode,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    sm: Option<&StateMachine>,
    key: Option<Ustr>,
    user: Option<Ustr>,
    index: Option<usize>,
    service_key: Option<&Ustr>,
    notifier: Option<Notifier>,
    resources: &mut Resources,
  ) {
    if let Some(index) = index {
      if let Some(inst) = service.instances.get_mut(index) {
        self.stop_service_instance(
          inst,
          service.metadata.clone(),
          mode,
          dispatch,
          sm,
          key.clone(),
          user.clone(),
          resources,
        );
      }
    } else {
      for inst in service.instances.iter_mut() {
        self.stop_service_instance(
          inst,
          service.metadata.clone(),
          mode,
          dispatch,
          sm,
          key.clone(),
          user.clone(),
          resources,
        );
      }
    }

    if service
      .instances
      .iter()
      .filter(|x| x.state == ServiceState::Active)
      .count()
      < 1
    {
      service.last_state = ServiceState::Stopping;

      let mut fields = self.log_fields(service, "stop");
      fields.insert("mode".to_string(), format!("{mode:?}"));
      if let Some(ref key) = key {
        fields.insert("key".to_string(), format!("{key}"));
      };
      if let Some(ref user) = user {
        fields.insert("user".to_string(), user.to_string());
      };
      log.log(
        LogLevel::Info,
        "service-runtime",
        "service stopping",
        fields,
      );

      let full_name = service_key
        .cloned()
        .unwrap_or(service.metadata.name.clone())
        .strip_prefix("units:")
        .map(|n| n.to_ustr())
        .unwrap_or(service.metadata.name.clone());

      let _ = dispatch.dispatch(
        "sockets",
        "clear_for",
        RuntimePayload::default().insert("name", full_name.clone()),
      );

      let _ = dispatch.dispatch(
        "services",
        "reconcile_stacks",
        rpayload!({
          "service": service_key.cloned().unwrap_or(service.metadata.name.clone()),
          "id": service.id.0,
          "action": ServiceEventKind::Stopped
        }),
      );

      let _ = dispatch.dispatch(
        "timer",
        "reconcile_timers",
        rpayload!({
          "service": service_key.cloned().unwrap_or(service.metadata.name.clone()),
          "id": service.id.0,
          "action": ServiceEventKind::Stopped
        }),
      );

      if let Some(ref notifier) = notifier {
        let _ = notifier.notify();
      }

      // if let Some(service_key) = service_key {
      //   let full_name = if service_key.starts_with("units:") {
      //     service_key.strip_prefix("units:").unwrap_or("").to_ustr()
      //   } else {
      //     service_key.clone()
      //   };
      //   let _ = dispatch.dispatch(
      //     "sockets",
      //     "resume_fds",
      //     RuntimePayload::default().insert("name", full_name),
      //   );
      // }
    }
  }

  fn handle_child_exit(
    &mut self,
    service: &mut Service,
    pid: i32,
    code: i32,
    _log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    sm: Option<&StateMachine>,
    service_key: Ustr,
    resources: &mut Resources,
  ) -> Option<ServiceExitAction> {
    self.disarm_watchdog_pid(pid as u32, resources);
    let idx = service.instances.find_by_pid(pid)?;
    let (manually_stopped, retry_count) = {
      let inst = &mut service.instances.0[idx];

      if matches!(inst.state, ServiceState::Active | ServiceState::Stopping) {
        self.run_triggers(service.metadata.on_stop.as_ref(), sm, dispatch);
      }

      inst.state = ServiceState::Exited(code);
      inst.child = None;
      (inst.manually_stopped, inst.retry_count)
    };

    service.last_state = ServiceState::Exited(code);

    self.maybe_unregister_stdio_transport(service, dispatch);

    let restart_policy = service.metadata.restart.as_ref();
    let action = if manually_stopped {
      ServiceExitAction::StopDependents
    } else {
      match restart_policy {
        Some(RestartPolicy::Bool(true)) => ServiceExitAction::Restart,
        Some(RestartPolicy::OnFailure { max_retries }) => {
          if code != 0 && *max_retries > 0 && retry_count < *max_retries {
            if let Some(inst) = service.instances.0.get_mut(idx) {
              inst.retry_count += 1;
            }
            ServiceExitAction::Restart
          } else {
            ServiceExitAction::StopDependents
          }
        }
        _ => ServiceExitAction::StopDependents,
      }
    };

    if !matches!(action, ServiceExitAction::Restart) {
      service.instances.0.retain(|inst| {
        inst.state == ServiceState::Active
          || inst.state == ServiceState::Starting
          || inst.state == ServiceState::Stopping
      });

      let full_name = service_key.strip_prefix("units:")?.to_ustr();
      let _ = dispatch.dispatch(
        "sockets",
        "clear_for",
        RuntimePayload::default().insert("name", full_name.clone()),
      );

      let _ = dispatch.dispatch(
        "sockets",
        "resume_fds",
        RuntimePayload::default().insert("name", full_name),
      );
    }

    Some(action)
  }

  // fn timeout_sweep(&mut self, service: &mut Service) {
  //   for inst in service.instances.iter_mut() {
  //     if inst.state == ServiceState::Stopping {
  //       if let Some(stop_time) = inst.stop_time {
  //         if stop_time.elapsed() > std::time::Duration::from_secs(5) {
  //           if let Some(child) = inst.child.as_ref() {
  //             let pgid = Pid::from_raw(-(child.id() as i32));
  //             let _ = kill(pgid, Signal::SIGKILL);
  //           }
  //         }
  //       }
  //     }
  //   }
  // }

  // fn timeout_sweep(
  //   &mut self,
  //   service: &mut Service,
  //   log: &LogHandle,
  //   dispatch: &RuntimeDispatcher,
  // ) {
  //   let mut should_stop = false;

  //   for inst in service.instances.iter_mut() {
  //       if Instant::now() >= deadline {
  //         should_stop = true;
  //         break;
  //       }
  //     }
  //   }

  //   if should_stop {
  //     self.stop_service(service, StopMode::Graceful, log, dispatch, None, None, None);
  //   }
  // }

  fn run_triggers(
    &self,
    triggers: Option<&Vec<Trigger>>,
    sm: Option<&StateMachine>,
    dispatch: &RuntimeDispatcher,
  ) {
    if let Some(triggers) = triggers {
      trigger_events(triggers.clone(), sm, dispatch);
    }
  }

  fn register_stdio_transport(
    &self,
    service: &Service,
    dispatch: &RuntimeDispatcher,
    unit: Option<String>,
  ) {
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
      rpayload!({ "endpoint": unit.map(|unit| format!("{unit}:{}", service.metadata.name)).unwrap_or(service.metadata.name.to_string()).to_ustr() }),
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
      rpayload!({ "endpoint": service.metadata.name.to_string() }),
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

  fn check_branch_match(
    &self,
    spec: &str,
    key: &str,
    sm: &StateMachine,
    vh: Option<&VariableHeap>,
  ) -> bool {
    let key_val = serde_json::json!(key);

    if let Some(var_name) = spec.strip_prefix("var:") {
      if let Some(vh) = vh {
        if let Some(val) = vh.get(var_name) {
          if let Ok(json_val) = serde_json::to_value(val) {
            return crate::triggers::subset_match(&key_val, &json_val);
          }
        }
      }
      return false;
    }

    let state_name = spec.strip_prefix("state:").unwrap_or(spec);
    if let Some(instances) = sm.states.get(&Ustr::from(state_name)) {
      for inst in instances {
        if crate::triggers::subset_match(&key_val, &inst.payload.to_json()) {
          return true;
        }
      }
    }
    false
  }
}

#[derive(Debug)]
enum ServiceExitAction {
  Restart,
  StopDependents,
}

fn is_stdio_transport(method: &TransportMethod) -> bool {
  match method {
    TransportMethod::Type(id) => id.0.as_str() == "stdio",
    TransportMethod::Options { id, .. } => id.0.as_str() == "stdio",
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

fn start_service_stream_logs(service_name: Ustr, child: &mut Child, log: LogHandle) {
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
        fields.insert("service".to_string(), service_name.to_string());
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
        fields.insert("service".to_string(), service_name.to_string());
        fields.insert("stream".to_string(), "stderr".to_string());
        log.log(LogLevel::Warn, "service-output", line, fields);
      }
    });
  }
}

fn start_stdin_writer(
  service_name: Ustr,
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
        fields.insert("service".to_string(), service_name.to_string());
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
  pub service: Option<Ustr>,
  pub state: Option<Ustr>,
  pub flow_type: Option<FlowType>,
  pub payload: Option<FlowPayload>,
  pub action: FlowAction,
}

#[derive(Debug, Clone, Default)]
pub struct ServiceBranchContext {
  pub key: Option<Ustr>,
  pub payload: Option<FlowPayload>,
  pub forced_user: Option<Ustr>,
}

impl Into<serde_json::Value> for EmitTrigger {
  fn into(self) -> serde_json::Value {
    serde_json::to_value(self).unwrap_or_default()
  }
}

#[derive(Debug, Clone, Default)]
pub struct SocketActivation {
  pub fds: Vec<RawFd>,
  pub names: Vec<Ustr>,
}

impl SocketActivation {
  pub fn is_empty(&self) -> bool {
    self.fds.is_empty()
  }
}

impl From<Vec<(Ustr, RawFd)>> for SocketActivation {
  fn from(value: Vec<(Ustr, RawFd)>) -> Self {
    SocketActivation {
      fds: value.iter().map(|(_, x)| *x).collect(),
      names: value.into_iter().map(|(x, _)| x).collect(),
    }
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
    .parse_payload::<SSPayload>()
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
    if let Some(ref perms) = svc.managed_by {
      perms
        .iter()
        .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
    } else {
      match &svc.space {
        ServiceSpace::User => true,
        ServiceSpace::UserSelective { user } => user.as_str() == caller.username.as_str(),
        ServiceSpace::System => false,
      }
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
    rpayload!({ "name": payload.name.to_ustr(), "only_user": only_user }),
  );

  if payload.persist {
    let _ = dispatch.dispatch(
      "flow",
      "set_state",
      FlowRuntimePayload::new("rind:active")
        .payload(payload.name.clone())
        .into(),
    );
  }

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
    .parse_payload::<SSPayload>()
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
    if let Some(ref perms) = svc.managed_by {
      perms
        .iter()
        .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
    } else {
      match &svc.space {
        ServiceSpace::User => true,
        ServiceSpace::UserSelective { user } => user.as_str() == caller.username.as_str(),
        ServiceSpace::System => false,
      }
    }
  } else {
    false
  };

  if !can_manage {
    return Err(CoreError::PermissionDenied);
  }

  let force = payload.force;
  let only_user = if uid == 0 || pm.user_has(uid, PERM_SYSTEM_SERVICES) {
    None
  } else {
    caller.map(|u| u.username.clone())
  };
  let _ = dispatch.dispatch(
    "services",
    "stop",
    rpayload!({ "name": payload.name.to_ustr(), "force": force, "only_user": only_user }),
  );

  if payload.persist {
    let _ = dispatch.dispatch(
      "flow",
      "remove_state",
      FlowRuntimePayload::new("rind:active")
        .payload(payload.name.clone())
        .into(),
    );
  }

  Ok(Message::ok(format!("stopped {}", payload.name)))
}

impl Runtime for ServiceRuntime {
  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "bootstrap" => {
        self.rebuild_trigger_index(ctx.registry.metadata);
      }
      "watch_events" => {
        self.event_rx = Some(ctx.event_bus.subscribe::<rind_core::prelude::FlowEvent>());
      }
      "send_stdio" => {
        let endpoint = payload.get::<String>("endpoint")?;
        let message = payload.get::<TransportMessage>("message")?;
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
            let _ = dispatch.dispatch(
              "services",
              "evaluate_triggers",
              RuntimePayload::default().insert("trigger", trig),
            );
          }
        }

        while let Ok((service_name, message)) = self.stdio_rx.try_recv() {
          if message.name.as_ref().map(|x| x.as_str()) == Some("watchdog") {
            let _ = dispatch.dispatch(
              "services",
              "watchdog_ping",
              rpayload!({ "service": service_name.clone() }),
            );
          }
          if message.name.as_ref().map(|x| x.as_str()) == Some("log") {
            let (level, message_text, fields) = self.stdio_log_entry(&service_name, &message);
            log.log(level, "service-transport", message_text, fields);
            continue;
          }
          let _ = dispatch.dispatch(
            "transport",
            "ingest",
            rpayload!({
              "endpoint": service_name,
              "message": message
            }),
          );
        }
      }
      "watchdog_ping" => {
        let mut service = payload.get::<Ustr>("service")?;
        if !service.starts_with("units:") {
          service = Ustr::from(format!("units:{}", service));
        }
        let branch = payload.get::<Ustr>("branch").ok();
        let fds: Vec<RawFd> = self
          .watchdog_fds
          .iter()
          .filter_map(|(fd, binding)| {
            if binding.service_key != service {
              return None;
            }
            if let Some(branch) = &branch
              && binding.branch.as_ref() != Some(branch)
            {
              return None;
            }
            Some(*fd)
          })
          .collect();

        if let Some(service_meta) = ctx.registry.metadata.find::<Service>(
          "units",
          service.strip_prefix("units:").unwrap_or(service.as_str()),
        ) && let Some(watchdog) = &service_meta.watchdog
        {
          for fd in fds {
            let _ = self.refresh_watchdog_fd(fd, watchdog);
          }
        }
      }
      "watchdog_expired" => {
        let fd = payload.get::<i32>("fd")?;
        let Some(binding) = self.watchdog_fds.get(&(fd as RawFd)).cloned() else {
          return Ok(None);
        };
        let action = ctx
          .registry
          .metadata
          .find::<Service>(
            "units",
            binding
              .service_key
              .strip_prefix("units:")
              .unwrap_or(binding.service_key.as_str()),
          )
          .and_then(|svc| svc.watchdog.as_ref().map(|wd| wd.action))
          .unwrap_or(WatchdogAction::Restart);

        ctx.resources.terminate(fd);
        self.watchdog_fds.remove(&(fd as RawFd));
        self.watchdog_pids.remove(&binding.pid);

        match action {
          WatchdogAction::Signal => {
            let _ = kill(Pid::from_raw(-(binding.pid as i32)), Signal::SIGABRT);
          }
          WatchdogAction::Stop => {
            let _ = dispatch.dispatch(
              "services",
              "stop",
              rpayload!({
                "name": binding.service_key.strip_prefix("units:").unwrap_or(binding.service_key.as_str()).to_ustr(),
                "force": true,
                "only_user": binding.user.clone()
              }),
            );
          }
          WatchdogAction::Restart => {
            let _ = dispatch.dispatch(
              "services",
              "stop",
              rpayload!({
                "name": binding.service_key.strip_prefix("units:").unwrap_or(binding.service_key.as_str()).to_ustr(),
                "force": true,
                "only_user": binding.user.clone()
              }),
            );
            let _ = dispatch.dispatch(
              "services",
              "start",
              rpayload!({
                "name": binding.service_key.strip_prefix("units:").unwrap_or(binding.service_key.as_str()).to_ustr(),
                "only_user": binding.user
              }),
            );
          }
        }
      }
      "evaluate_triggers" => {
        let emit_trig = payload.get::<EmitTrigger>("trigger").unwrap_or_default();

        if self.trigger_index.is_empty() {
          self.rebuild_trigger_index(ctx.registry.metadata);
        }

        let sockets_map = get_all_sockets(&ctx.registry);
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, vh)| {
              let target_keys = if let Some(event_name) = emit_trig.state.as_ref() {
                self
                  .trigger_index
                  .get(event_name)
                  .cloned()
                  .unwrap_or_default()
              } else {
                registry
                  .metadata
                  .items::<Service>("units")
                  .unwrap_or_default()
                  .into_iter()
                  .map(|(group, meta)| Ustr::from(format!("{}:{}", group, meta.name)))
                  .collect::<HashSet<Ustr>>()
              };

              let emit_event = match (
                emit_trig.state.as_ref(),
                emit_trig.flow_type,
                emit_trig.payload.as_ref(),
              ) {
                (Some(name), Some(flow_type), Some(payload)) => Some(FlowInstance {
                  name: name.clone().into(),
                  payload: payload.clone(),
                  r#type: flow_type,
                }),
                _ => None,
              };

              for service_name in target_keys {
                let mut is_running = false;
                // println!("{service_name} at {:?}", emit_trig.action);

                let Some(meta) = registry
                  .metadata
                  .find::<Service>("units", service_name.as_str())
                else {
                  continue;
                };

                let Some((unit, _)) = service_name.split_once(':') else {
                  continue;
                };

                let service_key = Ustr::from(format!("units:{}", service_name));

                if let Some(instances) = registry.instances.get_mut(&service_key) {
                  for instance in instances.iter_mut() {
                    if let Some(service) = instance.downcast_mut::<Service>() {
                      is_running = service.instances.iter().any(|i| {
                        i.state == ServiceState::Active || i.state == ServiceState::Starting
                      });

                      if let Some(ref branching) = service.metadata.branching {
                        match (emit_trig.action, emit_event.as_ref(), is_running) {
                          (FlowAction::Revert, Some(event), true)
                            if branching.key.is_some() && event.name == branching.source_state =>
                          {
                            let key = Self::branch_key_from_payload(
                              &event.payload,
                              branching.key.as_deref(),
                            );

                            let to_stop: Vec<Ustr> = service
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
                                Some(sm),
                                Some(i),
                                None,
                                None,
                                Some(&service_key),
                                ctx.notifier.clone(),
                                ctx.resources,
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
                                  && !crate::flow::condition_is_active(sm, cond, None)
                              })
                            }
                            _ => false,
                          }
                        } else {
                          false
                        };

                        if (should_stop || auto_stop_on_revert) && is_running {
                          self.stop_service(
                            service,
                            StopMode::Graceful,
                            log,
                            dispatch,
                            Some(sm),
                            None,
                            None,
                            None,
                            Some(&service_key),
                            ctx.notifier.clone(),
                            ctx.resources,
                          );
                        }
                      }
                    }
                  }
                }

                let should_start = meta
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

                if !meta.branching.as_ref().is_some() && is_running {
                  continue;
                }

                let ser =
                  registry.instantiate_one("units", &format!("{unit}:{}", meta.name), |x| {
                    Ok(Service::new(x))
                  })?;

                if let Some(branching) = &ser.metadata.branching {
                  let mut branches = sm
                    .states
                    .get(&branching.source_state)
                    .cloned()
                    .unwrap_or_default();

                  // this might allow for signal branching
                  if let Some(event) = emit_event.as_ref() {
                    if event.r#type == FlowType::Signal && event.name == branching.source_state {
                      branches.push(event.clone());
                    }
                  }

                  let mut started = 0usize;
                  for branch in branches {
                    let Some(key) =
                      Self::branch_key_from_payload(&branch.payload, branching.key.as_deref())
                    else {
                      continue;
                    };

                    if ser.instances.iter().any(|i| {
                      i.key == key
                        && (i.state == ServiceState::Active || i.state == ServiceState::Starting)
                    }) {
                      continue;
                    }

                    if let Some(onlys) = &branching.only {
                      let mut matched = false;
                      for spec in onlys {
                        if self.check_branch_match(spec, key.as_str(), sm, Some(vh)) {
                          matched = true;
                          break;
                        }
                      }
                      if !matched {
                        continue;
                      }
                    }

                    if let Some(excepts) = &branching.except {
                      let mut skipped = false;
                      for spec in excepts {
                        if self.check_branch_match(spec, key.as_str(), sm, Some(vh)) {
                          skipped = true;
                          break;
                        }
                      }
                      if skipped {
                        continue;
                      }
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
                    match self.spawn_all(
                      ser,
                      log,
                      dispatch,
                      Some(&branch_ctx),
                      &sockets_map,
                      Some(sm),
                      Some(vh),
                      service_key.clone().into(),
                      ctx.notifier.clone(),
                      ctx.resources,
                    ) {
                      Ok(instances) => {
                        ser.instances.extend(instances);
                        self.register_stdio_transport(ser, dispatch, None);

                        if !is_running {
                          if let Some(inst) = ser.instances.as_one_mut() {
                            inst.state = ServiceState::Active;
                            self.run_triggers(ser.metadata.on_start.as_ref(), Some(sm), dispatch);
                          }

                          let _ = dispatch.dispatch(
                            "services",
                            "reconcile_stacks",
                            rpayload!({
                              "service": service_name.clone(),
                              "action": ServiceEventKind::Started
                            }),
                          );

                          let _ = dispatch.dispatch(
                            "timer",
                            "reconcile_timers",
                            rpayload!({
                              "service": service_name.clone(),
                              "action": ServiceEventKind::Started
                            }),
                          );
                        }

                        started += 1;
                        log.log(
                          LogLevel::Info,
                          "service-runtime",
                          "started branched service instance",
                          [
                            ("service".to_string(), service_name.to_string()),
                            ("branch".into(), key.to_string()),
                          ]
                          .into(),
                        );
                      }
                      Err(e) => {
                        let mut fields = self.log_fields(ser, "start");
                        fields.insert("branch".into(), key.to_string());
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

                self.start_service(
                  ser,
                  log,
                  &sockets_map,
                  Some(sm),
                  dispatch,
                  Some(vh),
                  service_key.into(),
                  ctx.notifier.clone(),
                  ctx.resources,
                );
              }
              Ok(())
            },
          )?;
      }
      "start" => {
        let socket_fds = payload
          .get::<Vec<i32>>("socket_fds")
          .ok()
          .unwrap_or_default()
          .into_iter()
          .map(|fd| fd as RawFd)
          .collect::<Vec<_>>();
        let socket_fd_names = payload
          .get::<Vec<Ustr>>("socket_fd_names")
          .ok()
          .unwrap_or_default();
        let mut sockets_map = get_all_sockets(&ctx.registry);
        let name = payload.get::<Ustr>("name")?;
        if !socket_fds.is_empty() {
          let entry = sockets_map
            .entry(name.clone())
            .or_insert_with(|| SocketActivation {
              fds: Vec::new(),
              names: Vec::new(),
            });
          entry.fds.extend(socket_fds);
          entry.names.extend(socket_fd_names);
        }
        let only_user = payload.get::<String>("only_user").ok();
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, vh)| {
              let service_key = format!("units:{}", name);
              let service = registry
                .instantiate_one::<Service>("units", name.clone(), |x| Ok(Service::new(x)))?;
              if let Some(user) = only_user.clone() {
                let launch_ctx = ServiceBranchContext {
                  key: None,
                  payload: None,
                  forced_user: Some(user.into()),
                };

                match self.spawn_all(
                  service,
                  log,
                  dispatch,
                  Some(&launch_ctx),
                  &sockets_map,
                  Some(sm),
                  Some(vh),
                  service_key.to_ustr(),
                  ctx.notifier.clone(),
                  ctx.resources,
                ) {
                  Ok(instances) => {
                    service.instances.extend(instances);
                    self.register_stdio_transport(service, dispatch, None);
                    if let Some(inst) = service.instances.as_one_mut() {
                      inst.state = ServiceState::Active;
                      self.run_triggers(service.metadata.on_start.as_ref(), Some(sm), dispatch);
                    }
                    let _ = dispatch.dispatch(
                      "services",
                      "reconcile_stacks",
                      rpayload!({
                        "service": service_key.to_ustr(),
                        "id": service.id.0,
                        "action": ServiceEventKind::Started
                      }),
                    );

                    let _ = dispatch.dispatch(
                      "timer",
                      "reconcile_timers",
                      rpayload!({
                        "service": service_key.to_ustr(),
                        "id": service.id.0,
                        "action": ServiceEventKind::Started
                      }),
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
                self.start_service(
                  service,
                  log,
                  &sockets_map,
                  Some(sm),
                  dispatch,
                  Some(vh),
                  service_key.into(),
                  ctx.notifier.clone(),
                  ctx.resources,
                );
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
                  rpayload!({ "endpoint": name }),
                );
              }
              Ok(())
            },
          )?;
      }
      "stop" => {
        let name = payload.get::<Ustr>("name")?;
        let force = payload.get::<bool>("force").unwrap_or(false);
        let index = payload.get::<usize>("index").ok();
        let mode = if force {
          StopMode::ForceKill
        } else {
          StopMode::Graceful
        };
        let notifier = ctx.notifier.clone();

        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, _)| {
              let service = registry
                .instantiate_one::<Service>("units", name.clone(), |x| Ok(Service::new(x)))?;
              let only_user = payload.get::<Ustr>("only_user").ok();

              self.stop_service(
                service,
                mode,
                log,
                dispatch,
                Some(sm),
                None,
                only_user,
                index,
                Some(&name),
                notifier,
                ctx.resources,
              );
              Ok(())
            },
          )?;
      }
      "stop_all" => {
        let force = payload.get::<bool>("force").unwrap_or(false);
        let mode = if force {
          StopMode::ForceKill
        } else {
          StopMode::Graceful
        };
        let notifier = ctx.notifier.clone();

        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, _)| {
              let keys: Vec<Ustr> = registry
                .instances
                .keys()
                .filter(|k| k.starts_with("units:"))
                .cloned()
                .collect();

              for key in keys {
                if let Some(instances) = registry.instances.get_mut(&key) {
                  for instance in instances.iter_mut() {
                    if let Some(service) = instance.downcast_mut::<Service>() {
                      self.stop_service(
                        service,
                        mode,
                        log,
                        dispatch,
                        Some(sm),
                        None,
                        None,
                        None,
                        Some(&key),
                        notifier.clone(),
                        ctx.resources,
                      );
                    }
                  }
                }
              }

              Ok(())
            },
          )?;
      }
      "start_all" => {
        let mut started: HashSet<Ustr> = HashSet::new();
        let mut pending: Vec<(Ustr, Vec<Ustr>, Arc<ServiceMetadata>)> = Vec::new();
        let sockets_map = get_all_sockets(&ctx.registry);
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, vh)| {
              let Some(active) = sm.states.get("rind:active") else {
                return Ok(());
              };

              let mut all_services: Vec<(Ustr, Arc<ServiceMetadata>)> = Vec::new();
              for branch in active {
                let name = Ustr::from(branch.payload.to_string_payload());
                if let Some(svc) = ctx
                  .registry
                  .metadata
                  .find::<Service>("units", name.as_str())
                {
                  all_services.push((name, svc));
                }
              }

              for (full_name, svc_meta) in &all_services {
                let service_key = Ustr::from(format!("units:{}", full_name));
                if let Some(afters) = &svc_meta.after {
                  pending.push((full_name.clone(), afters.clone(), svc_meta.clone()));
                } else {
                  let service =
                    match registry.instantiate_one::<Service>("units", full_name.as_str(), |x| {
                      Ok(Service::new(x))
                    }) {
                      Ok(service) => Ok(service),
                      Err(CoreError::MetadataNotFound(_)) => continue,
                      Err(e) => {
                        // log.log(
                        //   LogLevel::Error,
                        //   "service-runtime",
                        //   format!("failed to instantiate service '{full_name}': {e}"),
                        //   Default::default(),
                        // );
                        Err(e)
                      }
                    }?;
                  self.start_service(
                    service,
                    log,
                    &sockets_map,
                    Some(sm),
                    dispatch,
                    Some(vh),
                    service_key,
                    ctx.notifier.clone(),
                    ctx.resources,
                  );
                  started.insert(full_name.clone());
                }
              }

              loop {
                let mut progress = false;
                pending.retain(|(name, afters, _meta)| {
                  if afters.iter().all(|a| started.contains(a)) {
                    let service_key = Ustr::from(format!("units:{}", name));
                    if let Ok(service) =
                      registry
                        .instantiate_one::<Service>("units", name.clone(), |x| Ok(Service::new(x)))
                    {
                      self.start_service(
                        service,
                        log,
                        &sockets_map,
                        Some(sm),
                        dispatch,
                        Some(vh),
                        service_key,
                        ctx.notifier.clone(),
                        ctx.resources,
                      );
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
          let names: Vec<String> = pending.iter().map(|(n, _, _)| n.to_string()).collect();
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
        let service = normalize_uaddr(payload.get::<Ustr>("service")?, "units:");
        let action = payload.get::<ServiceEventKind>("action")?;
        let notifier = ctx.notifier.clone();

        let metadata = ctx
          .registry
          .metadata
          .metadata("units")
          .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;

        let mut dependents: Vec<(Ustr, Arc<ServiceMetadata>)> = Vec::new();
        for group in metadata.groups() {
          if let Some(svcs) = ctx
            .registry
            .metadata
            .group_items::<Service>("units", group.clone())
          {
            for svc in svcs {
              if let Some(ref dependencies) = svc.after
                && dependencies.contains(&service)
              {
                dependents.push((Ustr::from(format!("{group}:{}", svc.name)), svc));
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
            ctx
              .registry
              .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
                (StateMachine::KEY.into(), VariableHeap::KEY.into()),
                |registry, (sm, _)| {
                  for (dependent, _) in dependents {
                    if let Ok(service) = registry.as_one_mut::<Service>("units", dependent.as_str())
                    {
                      self.stop_service(
                        service,
                        StopMode::Graceful,
                        log,
                        dispatch,
                        Some(sm),
                        None,
                        None,
                        None,
                        Some(&dependent),
                        notifier.clone(),
                        ctx.resources,
                      );
                    }
                  }
                  Ok(())
                },
              )?;
          }
          ServiceEventKind::Started => {
            let sockets_map = get_all_sockets(&ctx.registry);
            ctx
              .registry
              .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
                (StateMachine::KEY.into(), VariableHeap::KEY.into()),
                |registry, (sm, vh)| {
                  for (dependent, svc) in dependents {
                    let should_start = svc.after.as_ref().unwrap().iter().any(|a| {
                      if let Ok(ref svc) = registry.as_one::<Service>("units", a.as_str()) {
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
                      let service_key = Ustr::from(format!("units:{}", dependent));
                      let service =
                        registry.instantiate_one::<Service>("units", dependent.as_str(), |x| {
                          Ok(Service::new(x))
                        })?;
                      self.start_service(
                        service,
                        log,
                        &sockets_map,
                        Some(sm),
                        dispatch,
                        Some(vh),
                        service_key.into(),
                        ctx.notifier.clone(),
                        ctx.resources,
                      );
                    }
                  }
                  Ok(())
                },
              )?;
          }
        }
      }
      "child_exited" => {
        let pid = payload.get::<i32>("pid")? as u32;
        let code = payload.get::<i32>("code")?;

        if let Some(service_key) = self.pid_map.remove(&pid) {
          self.stopping_map.remove(&pid);
          let sockets_map = get_all_sockets(&ctx.registry);

          ctx
            .registry
            .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
              (StateMachine::KEY.into(), VariableHeap::KEY.into()),
              |registry, (sm, vh)| {
                if let Some(instances) = registry.instances.get_mut(&service_key) {
                  for instance in instances.iter_mut() {
                    if let Some(service) = instance.downcast_mut::<Service>() {
                      if let Some(exit_action) = self.handle_child_exit(
                        service,
                        pid as i32,
                        code,
                        log,
                        dispatch,
                        Some(sm),
                        service_key.clone(),
                        ctx.resources,
                      ) {
                        match exit_action {
                          ServiceExitAction::Restart => {
                            self.start_service(
                              service,
                              log,
                              &sockets_map,
                              Some(sm),
                              dispatch,
                              Some(vh),
                              service_key.clone(),
                              ctx.notifier.clone(),
                              ctx.resources,
                            );
                          }
                          ServiceExitAction::StopDependents => {
                            let _ = dispatch.dispatch(
                              "services",
                              "reconcile_stacks",
                              rpayload!({
                                "service": service_key.clone(),
                                "id": service.id.0,
                                "action": ServiceEventKind::Exited { code }
                              }),
                            );
                            let _ = dispatch.dispatch(
                              "timer",
                              "reconcile_timers",
                              rpayload!({
                                "service": service_key.clone(),
                                "id": service.id.0,
                                "action": ServiceEventKind::Exited { code }
                              }),
                            );
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
      }
      "timeout_sweep" => {
        // let keys: Vec<String> = ctx
        //   .registry
        //   .instances
        //   .keys()
        //   .filter(|k| k.starts_with("units:"))
        //   .cloned()
        //   .collect();

        // for key in keys {
        //   if let Some(instances) = ctx.registry.instances.get_mut(&key) {
        //     for instance in instances.iter_mut() {
        //       if let Some(service) = instance.downcast_mut::<Service>() {
        //         if service
        //           .instances
        //           .iter()
        //           .any(|i| i.state == ServiceState::Stopping)
        //         {
        //           self.timeout_sweep(service);
        //         }
        //       }
        //     }
        //   }
        // }
        let now = Instant::now();
        let timeout = Duration::from_secs(5);
        let expired_pids: Vec<u32> = self
          .stopping_map
          .iter()
          .filter(|(_, stop_time)| now.duration_since(**stop_time) > timeout)
          .map(|(&pid, _)| pid)
          .collect();

        for pid in expired_pids {
          let pgid = Pid::from_raw(-(pid as i32));
          let _ = kill(pgid, Signal::SIGKILL);
          self.stopping_map.remove(&pid);
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
