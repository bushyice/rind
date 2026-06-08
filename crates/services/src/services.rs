use nix::sys::signal::{Signal, kill};
use nix::sys::time::TimeSpec;
use nix::sys::timerfd::{ClockId, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};
use nix::unistd::Pid;
use rind_ipc::payloads::SSPayload;
use rind_ipc::ser::ser_to_vec;
use rind_ipc::{Message, TransportMessageAction, TransportMessageType};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::ops::{Deref, DerefMut};
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use rind_core::reexports::*;
use rind_core::{notifier::Notifier, prelude::*};

use crate::sockets::get_all_sockets;
use crate::{SocketRuntime, TimerRuntime};
use rind_flow::transport::{TransportMethod, start_stdout_listener, transport_id};
use rind_flow::transport::{TransportRuntime, socket_path};
use rind_flow::triggers::{check_condition, subset_match, trigger_events};
use rind_flow::{
  EmitTrigger, FacetGraph, FlowInstance, FlowItem, FlowPayload, FlowRuntime, FlowType, Trigger,
  condition_is_active, condition_matches,
};
use rind_ipc::TransportMessage;
use rind_primitives::mounts::{Mount, NamespaceMountEntry};
use rind_primitives::permissions::PERM_SYSTEM_SERVICES;
use rind_primitives::scopes::ScopeStore;
use rind_primitives::variables::VariableHeap;

#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct RunOption {
  #[serde(default)]
  pub exec: Ustr,
  #[serde(default)]
  pub args: Vec<Ustr>,
  pub env: Option<HashMap<Ustr, Ustr>>,
  pub variable: Option<String>,
  #[serde(default)]
  pub executor: Option<Ustr>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum RunOptions {
  One(RunOption),
  Many(Vec<RunOption>),
}

impl Default for RunOptions {
  fn default() -> Self {
    RunOptions::One(RunOption::default())
  }
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
      .map(|x| {
        format!(
          "{} {}",
          x.exec,
          x.args
            .iter()
            .map(|a| a.as_str())
            .collect::<Vec<_>>()
            .join(" ")
        )
      })
      .collect::<Vec<String>>()
  }
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

use crate::executors::{
  Executor, ExecutorContext, ImaExecutor, InstanceHandle, NamespaceNetworkConfig, NativeExecutor,
  RemoteExecutor,
};

pub struct ChildInstance {
  pub key: Ustr,
  pub user: Option<Ustr>,
  pub handle: Option<Box<dyn InstanceHandle>>,
  pub state: ServiceState,
  pub retry_count: u32,
  pub stop_time: Option<Instant>,
  pub manually_stopped: bool,
}

impl ChildInstance {
  pub fn new(
    key: impl Into<Ustr>,
    user: Option<Ustr>,
    handle: Option<Box<dyn InstanceHandle>>,
  ) -> Self {
    Self {
      key: key.into(),
      user,
      handle,
      state: ServiceState::Active,
      retry_count: 0,
      stop_time: None,
      manually_stopped: false,
    }
  }

  pub fn pid(&self) -> Option<u32> {
    self.handle.as_ref().and_then(|h| h.pid())
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
      .position(|inst| inst.pid() == Some(pid as u32))
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
  pub source: Ustr,
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
  pub facet: Option<Ustr>,
  #[serde(default)]
  pub branch: bool,
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

impl Default for ServiceCgroup {
  fn default() -> Self {
    Self {
      path: None,
      memory_max: None,
      cpu_max: None,
      pids_max: None,
    }
  }
}

impl ServiceCgroup {
  fn is_empty(&self) -> bool {
    self.path.is_none()
      && self.memory_max.is_none()
      && self.cpu_max.is_none()
      && self.pids_max.is_none()
  }
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
  #[serde(default)]
  pub mount_private: bool,
  pub rootfs: Option<Ustr>,
  pub hostname: Option<Ustr>,
  #[serde(default)]
  pub persist: bool,
  #[serde(default)]
  pub init: bool,
}

impl ServiceNamespaces {
  fn is_empty(&self) -> bool {
    !self.mount
      && !self.uts
      && !self.ipc
      && !self.net
      && !self.pid
      && !self.user
      && !self.cgroup
      && !self.mount_private
      && self.rootfs.is_none()
      && self.hostname.is_none()
      && !self.persist
      && !self.init
  }
}

#[derive(Debug, Clone, Default)]
pub struct ServiceIsolation {
  pub scope: Option<Ustr>,
  pub cgroup: Option<ServiceCgroup>,
  pub namespaces: Option<ServiceNamespaces>,
  pub capabilities: Option<CapabilityPolicy>,
  pub seccomp: Option<SeccompPolicy>,
}

impl ServiceIsolation {
  pub fn needs_namespace_supervisor(&self) -> bool {
    self
      .namespaces
      .as_ref()
      .map(|ns| ns.pid || ns.user || ns.persist || ns.init)
      .unwrap_or(false)
      || self.capabilities.is_some()
      || self.seccomp.is_some()
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityPolicy {
  pub drop: Vec<Ustr>,
  pub keep: Vec<Ustr>,
}

impl CapabilityPolicy {
  fn is_empty(&self) -> bool {
    self.drop.is_empty() && self.keep.is_empty()
  }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeccompPolicy {
  pub profile: Option<Ustr>,
  pub path: Option<Ustr>,
}

impl SeccompPolicy {
  fn is_empty(&self) -> bool {
    self.profile.is_none() && self.path.is_none()
  }
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
    namespaces, watchdog, description
  ),
  derive_metadata(Debug, Default)
)]
pub struct Service {
  // Metadata
  pub name: Ustr,
  pub run: RunOptions,
  pub description: Option<String>,
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
  stdio_tx: Sender<(Ustr, TransportMessage, usize)>,
  stdio_rx: Receiver<(Ustr, TransportMessage, usize)>,
  stdio_writers: Mutex<HashMap<Ustr, Vec<Sender<TransportMessage>>>>,
  pid_map: HashMap<u32, Ustr>,
  stopping_map: HashMap<u32, Instant>,
  trigger_index: HashMap<Ustr, HashSet<Ustr>>,
  watchdog_fds: HashMap<RawFd, WatchdogBinding>,
  watchdog_pids: HashMap<u32, RawFd>,
  executors: HashMap<Ustr, Box<dyn Executor>>,
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
    let mut executors: HashMap<Ustr, Box<dyn Executor>> = HashMap::new();

    executors.insert(Ustr::from("native"), Box::new(NativeExecutor));
    executors.insert(Ustr::from("remote"), Box::new(RemoteExecutor));
    executors.insert(Ustr::from("ima"), Box::new(ImaExecutor));

    let _ =
      EXTENSIONS.with(|extensions| extensions.get().map(|e| e.act("collect", &mut executors)));

    Self {
      stdio_tx,
      stdio_rx,
      stdio_writers: Mutex::new(HashMap::new()),
      pid_map: HashMap::new(),
      stopping_map: HashMap::new(),
      trigger_index: HashMap::new(),
      watchdog_fds: HashMap::new(),
      watchdog_pids: HashMap::new(),
      executors,
    }
  }
}

impl ServiceRuntime {
  fn instance_key_name(key: &str) -> Ustr {
    Ustr::from(key.split('@').next().unwrap_or(key))
  }

  fn ensure_scoped_name(name: &str) -> Ustr {
    if name.contains('@') {
      Ustr::from(name)
    } else {
      Ustr::from(format!("{name}@static"))
    }
  }

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
    cgroup: Option<&ServiceCgroup>,
    branch_ctx: Option<&ServiceBranchContext>,
    user: Option<&Ustr>,
  ) -> Option<PathBuf> {
    let cgroup = cgroup?;
    let mut path = if let Some(custom) = &cgroup.path {
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

  fn attr<'a>(attrs: &'a HashMap<Ustr, String>, keys: &[&str]) -> Option<&'a str> {
    keys
      .iter()
      .find_map(|key| attrs.get(&Ustr::from(*key)).map(|v| v.as_str()))
  }

  fn attr_bool(attrs: &HashMap<Ustr, String>, keys: &[&str]) -> Option<bool> {
    let value = Self::attr(attrs, keys)?;
    match value.trim().to_ascii_lowercase().as_str() {
      "1" | "true" | "yes" | "on" => Some(true),
      "0" | "false" | "no" | "off" => Some(false),
      _ => None,
    }
  }

  fn scope_cgroup(scope: Option<&str>) -> Option<ServiceCgroup> {
    let attrs = ScopeStore::attrs_for_scope(scope?)?;
    let cgroup = ServiceCgroup {
      path: Self::attr(&attrs, &["cgroup.path"]).map(Ustr::from),
      memory_max: Self::attr(&attrs, &["cgroup.memory-max", "cgroup.memory_max"]).map(Ustr::from),
      cpu_max: Self::attr(&attrs, &["cgroup.cpu-max", "cgroup.cpu_max"]).map(Ustr::from),
      pids_max: Self::attr(&attrs, &["cgroup.pids-max", "cgroup.pids_max"]).map(Ustr::from),
    };
    (!cgroup.is_empty()).then_some(cgroup)
  }

  fn scope_namespaces(scope: Option<&str>) -> Option<ServiceNamespaces> {
    let attrs = ScopeStore::attrs_for_scope(scope?)?;
    let namespaces = ServiceNamespaces {
      mount: Self::attr_bool(&attrs, &["namespace.mount", "namespaces.mount", "ns.mount"])
        .unwrap_or(false),
      uts: Self::attr_bool(&attrs, &["namespace.uts", "namespaces.uts", "ns.uts"]).unwrap_or(false),
      ipc: Self::attr_bool(&attrs, &["namespace.ipc", "namespaces.ipc", "ns.ipc"]).unwrap_or(false),
      net: Self::attr_bool(&attrs, &["namespace.net", "namespaces.net", "ns.net"]).unwrap_or(false),
      pid: Self::attr_bool(&attrs, &["namespace.pid", "namespaces.pid", "ns.pid"]).unwrap_or(false),
      user: Self::attr_bool(&attrs, &["namespace.user", "namespaces.user", "ns.user"])
        .unwrap_or(false),
      cgroup: Self::attr_bool(
        &attrs,
        &["namespace.cgroup", "namespaces.cgroup", "ns.cgroup"],
      )
      .unwrap_or(false),
      mount_private: Self::attr_bool(
        &attrs,
        &[
          "namespace.mount-private",
          "namespace.mount_private",
          "namespaces.mount-private",
          "namespaces.mount_private",
          "ns.mount-private",
          "ns.mount_private",
        ],
      )
      .unwrap_or(false),
      rootfs: Self::attr(
        &attrs,
        &["namespace.rootfs", "namespaces.rootfs", "ns.rootfs"],
      )
      .map(Ustr::from),
      hostname: Self::attr(
        &attrs,
        &["namespace.hostname", "namespaces.hostname", "ns.hostname"],
      )
      .map(Ustr::from),
      persist: Self::attr_bool(
        &attrs,
        &["namespace.persist", "namespaces.persist", "ns.persist"],
      )
      .unwrap_or(false),
      init: Self::attr_bool(&attrs, &["namespace.init", "namespaces.init", "ns.init"])
        .unwrap_or(false),
    };
    (!namespaces.is_empty()).then_some(namespaces)
  }

  fn split_attr_list(value: &str) -> Vec<Ustr> {
    value
      .split(',')
      .map(str::trim)
      .filter(|v| !v.is_empty())
      .map(Ustr::from)
      .collect()
  }

  fn scope_capabilities(scope: Option<&str>) -> Option<CapabilityPolicy> {
    let attrs = ScopeStore::attrs_for_scope(scope?)?;
    let caps = CapabilityPolicy {
      drop: Self::attr(
        &attrs,
        &["capabilities.drop", "capability.drop", "caps.drop"],
      )
      .map(Self::split_attr_list)
      .unwrap_or_default(),
      keep: Self::attr(
        &attrs,
        &["capabilities.keep", "capability.keep", "caps.keep"],
      )
      .map(Self::split_attr_list)
      .unwrap_or_default(),
    };
    (!caps.is_empty()).then_some(caps)
  }

  fn scope_seccomp(scope: Option<&str>) -> Option<SeccompPolicy> {
    let attrs = ScopeStore::attrs_for_scope(scope?)?;
    let seccomp = SeccompPolicy {
      profile: Self::attr(&attrs, &["seccomp.profile"]).map(Ustr::from),
      path: Self::attr(&attrs, &["seccomp.path"]).map(Ustr::from),
    };
    (!seccomp.is_empty()).then_some(seccomp)
  }

  fn merge_cgroup(
    scope: Option<ServiceCgroup>,
    service: Option<ServiceCgroup>,
  ) -> Option<ServiceCgroup> {
    match (scope, service) {
      (None, None) => None,
      (Some(c), None) | (None, Some(c)) => Some(c),
      (Some(scope), Some(service)) => Some(ServiceCgroup {
        path: service.path.or(scope.path),
        memory_max: service.memory_max.or(scope.memory_max),
        cpu_max: service.cpu_max.or(scope.cpu_max),
        pids_max: service.pids_max.or(scope.pids_max),
      }),
    }
  }

  fn merge_namespaces(
    scope: Option<ServiceNamespaces>,
    service: Option<ServiceNamespaces>,
  ) -> Option<ServiceNamespaces> {
    match (scope, service) {
      (None, None) => None,
      (Some(ns), None) | (None, Some(ns)) => Some(ns),
      (Some(scope), Some(service)) => Some(ServiceNamespaces {
        mount: scope.mount || service.mount,
        uts: scope.uts || service.uts,
        ipc: scope.ipc || service.ipc,
        net: scope.net || service.net,
        pid: scope.pid || service.pid,
        user: scope.user || service.user,
        cgroup: scope.cgroup || service.cgroup,
        mount_private: scope.mount_private || service.mount_private,
        rootfs: service.rootfs.or(scope.rootfs),
        hostname: service.hostname.or(scope.hostname),
        persist: scope.persist || service.persist,
        init: scope.init || service.init,
      }),
    }
  }

  fn validate_service_inline_namespaces(service: &Service) -> CoreResult<Void> {
    let Some(ns) = &service.metadata.namespaces else {
      return Ok(Void);
    };
    let mut invalid = Vec::new();
    if ns.pid {
      invalid.push("pid");
    }
    if ns.user {
      invalid.push("user");
    }
    if ns.persist {
      invalid.push("persist");
    }
    if ns.init {
      invalid.push("init");
    }
    if invalid.is_empty() {
      return Ok(Void);
    }

    Err(CoreError::InvalidState(format!(
      "service '{}' declares scope-only namespace feature(s) inline: {}; put them on scope attributes instead",
      service.metadata.name,
      invalid.join(", ")
    )))
  }

  fn isolation_for(service: &Service, scope: Option<&str>) -> CoreResult<ServiceIsolation> {
    Self::validate_service_inline_namespaces(service)?;
    Ok(ServiceIsolation {
      scope: scope.map(Ustr::from),
      cgroup: Self::merge_cgroup(Self::scope_cgroup(scope), service.metadata.cgroup.clone()),
      namespaces: Self::merge_namespaces(
        Self::scope_namespaces(scope),
        service.metadata.namespaces.clone(),
      ),
      capabilities: Self::scope_capabilities(scope),
      seccomp: Self::scope_seccomp(scope),
    })
  }

  fn setup_cgroup_for_pid(
    &self,
    service: &Service,
    cgroup: Option<&ServiceCgroup>,
    branch_ctx: Option<&ServiceBranchContext>,
    user: Option<&Ustr>,
    pid: u32,
  ) -> CoreResult<Void> {
    let Some(cgroup) = cgroup else {
      return Ok(Void);
    };
    let Some(path) = Self::cgroup_path_for(service, Some(cgroup), branch_ctx, user) else {
      return Ok(Void);
    };

    std::fs::create_dir_all(&path)?;

    if let Some(mem) = &cgroup.memory_max {
      let _ = std::fs::write(path.join("memory.max"), mem.as_str());
    }
    if let Some(cpu) = &cgroup.cpu_max {
      let _ = std::fs::write(path.join("cpu.max"), cpu.as_str());
    }
    if let Some(pids) = &cgroup.pids_max {
      let _ = std::fs::write(path.join("pids.max"), pids.as_str());
    }

    std::fs::write(path.join("cgroup.procs"), pid.to_string())?;
    Ok(Void)
  }

  fn arm_watchdog_timer(
    &mut self,
    service_key: Ustr,
    branch: Option<Ustr>,
    user: Option<Ustr>,
    pid: u32,
    watchdog: &ServiceWatchdog,
    resources: &mut Resources,
  ) -> Result<Void, CoreError> {
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
    Ok(Void)
  }

  fn disarm_watchdog_pid(&mut self, pid: u32, resources: &mut Resources) {
    if let Some(fd) = self.watchdog_pids.remove(&pid) {
      self.watchdog_fds.remove(&fd);
      resources.terminate(fd);
    }
  }

  fn refresh_watchdog_fd(&self, fd: RawFd, watchdog: &ServiceWatchdog) -> Result<Void, CoreError> {
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
    Ok(Void)
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
    service: &Service,
    source: &ServiceUserSource,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&FacetGraph>,
  ) -> CoreResult<Option<Ustr>> {
    let Some(sm) = sm else {
      return Ok(None);
    };
    if let Some(payload) = branch_ctx.and_then(|ctx| ctx.payload.as_ref())
      && let Some(user) = Self::payload_field_as_key(payload, &source.username_field)
      && source.branch
    {
      return Ok(Some(user));
    }

    let Some(facet) = &source.facet else {
      return Ok(None);
    };

    let Some(branches) = sm.facets.get(facet) else {
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
          if service
            .instances
            .iter()
            .any(|x| x.user != Some(user.clone()))
          {
            matches.insert(user);
            break;
          }
        }
      }
      if matches.is_empty() {
        return Ok(None);
      }
      if matches.len() > 1 {
        return Err(CoreError::InvalidState(format!(
          "ambiguous users for state '{}' using match key '{}'",
          facet, field
        )));
      }
      return Ok(matches.into_iter().next());
    }

    let mut users = HashSet::new();
    for branch in branches {
      if let Some(user) = Self::payload_field_as_key(&branch.payload, &source.username_field) {
        if service
          .instances
          .iter()
          .any(|x| x.user != Some(user.clone()))
        {
          users.insert(user);
          break;
        }
      }
    }
    if users.is_empty() {
      return Ok(None);
    }
    if users.len() > 1 {
      return Err(CoreError::InvalidState(format!(
        "ambiguous users in state '{}' (set user-source.match-branch-key)",
        facet
      )));
    }
    Ok(users.into_iter().next())
  }

  fn resolve_service_user(
    &self,
    service: &Service,
    branch_ctx: Option<&ServiceBranchContext>,
    sm: Option<&FacetGraph>,
    scope: Option<&str>,
  ) -> CoreResult<Option<Ustr>> {
    if let Some(user) = branch_ctx.and_then(|ctx| ctx.forced_user.as_ref()) {
      return Ok(Some(user.clone()));
    }

    let res = match &service.metadata.space {
      ServiceSpace::System => Ok(None),
      ServiceSpace::UserSelective { user } => Ok(Some(user.clone())),
      ServiceSpace::User => {
        if let Some(scope) = scope
          && let Some(user) = ScopeStore::user_for_scope(scope)
        {
          return Ok(Some(user));
        }

        if let Some(source) = &service.metadata.user_source {
          let user = self.resolve_user_from_source(service, source, branch_ctx, sm)?;
          if let Some(user) = user {
            return Ok(Some(user));
          }
        }

        if let Some(user) = branch_ctx.and_then(|ctx| ctx.key.as_ref()) {
          return Ok(Some(user.clone()));
        }

        if let Some(sm) = sm
          && let Some(sessions) = sm.facets.get("rind:user_session")
        {
          let mut users = HashSet::new();
          for sess in sessions {
            if let Some(user) = Self::payload_field_as_key(&sess.payload, "username") {
              if service
                .instances
                .iter()
                .any(|x| x.user != Some(user.clone()))
              {
                users.insert(user);
                break;
              }
            }
          }
          if users.len() == 1 {
            return Ok(users.into_iter().next());
          }
          if users.len() > 1 {
            return Err(CoreError::InvalidState(format!(
              "service '{}' is userspace but username is ambiguous; configure `user-source`",
              service.metadata.name
            )));
          }
        }

        Ok(None)
      }
    };
    res.or_else(|err| Err(err))
  }

  fn resolve_scope_default_user(&self, service: &Service, scope: Option<&str>) -> Option<Ustr> {
    if !matches!(service.metadata.space, ServiceSpace::System) {
      return None;
    }
    let scope = scope?;
    ScopeStore::user_for_scope(scope)
  }

  fn rebuild_trigger_index(&mut self, metadata: &MetadataRegistry) {
    self.trigger_index.clear();

    for (scope, services) in metadata.all_items::<Service>() {
      for (group, meta) in services {
        let key = Ustr::from(format!("{}:{}@{}", group, meta.name, scope));

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
  }

  pub fn spawn_all(
    &mut self,
    service: &Service,
    log: &LogHandle,
    branch_ctx: Option<&ServiceBranchContext>,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&FacetGraph>,
    variable_heap: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
    namespace_mounts: Vec<NamespaceMountEntry>,
    namespace_networks: Vec<NamespaceNetworkConfig>,
  ) -> CoreResult<Vec<ChildInstance>> {
    let mut instances = Vec::new();

    if let Some(sm) = sm {
      let key = Self::instance_key_name(registry_key.as_str());

      if let Some(inst) = sm.facets.get("rind:inactive")
        && inst
          .iter()
          .any(|x| x.payload.to_string_payload() == key.as_str())
      {
        return Ok(instances);
      }
    }

    for run in service.metadata.run.as_many() {
      let resolved = self.resolve_run_option(run, variable_heap);
      let run_ref = resolved.as_ref().unwrap_or(run);
      let instance = self.spawn_process(
        service,
        run_ref,
        log,
        branch_ctx,
        sockets_map,
        sm,
        variable_heap,
        registry_key.clone(),
        notifier.clone(),
        resources,
        namespace_mounts.clone(),
        namespace_networks.clone(),
      )?;

      instances.push(instance);
    }
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
        .filter_map(|(k, v)| v.as_str().map(|s| (Ustr::from(k.as_str()), Ustr::from(s))))
        .collect()
    });

    Some(RunOption {
      exec: Ustr::from(exec),
      args,
      env,
      variable: None,
      executor: None,
    })
  }

  pub fn spawn_service(
    &mut self,
    service: &mut Service,
    log: &LogHandle,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&FacetGraph>,
    variable_heap: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
    namespace_mounts: Vec<NamespaceMountEntry>,
    namespace_networks: Vec<NamespaceNetworkConfig>,
  ) -> CoreResult<Void> {
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service started",
      self.log_fields(service, "start"),
    );

    let instances = self.spawn_all(
      service,
      log,
      None,
      sockets_map,
      sm,
      variable_heap,
      registry_key,
      notifier,
      resources,
      namespace_mounts,
      namespace_networks,
    )?;
    service.instances.extend(instances);
    Ok(Void)
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
    branch_ctx: Option<&ServiceBranchContext>,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&FacetGraph>,
    variables: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
    namespace_mounts: Vec<NamespaceMountEntry>,
    namespace_networks: Vec<NamespaceNetworkConfig>,
  ) -> CoreResult<ChildInstance> {
    let (full_name, scope_name) = {
      let key = registry_key.as_str();
      if let Some((name, scope)) = key.rsplit_once('@') {
        (Ustr::from(name), Some(scope))
      } else {
        (registry_key.clone(), None)
      }
    };
    let mut args = run.args.clone();
    let mut envs = run.env.clone().unwrap_or_default();
    let branch_key = branch_ctx.and_then(|ctx| ctx.key.as_ref());
    let resolved_user = if let Some(u) = self.resolve_scope_default_user(service, scope_name) {
      Some(u)
    } else {
      self.resolve_service_user(service, branch_ctx, sm, scope_name)?
    };
    let isolation = Self::isolation_for(service, scope_name)?;
    let watchdog_cfg = service.metadata.watchdog.clone();

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
            sm.facets
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
          TransportMethod::Options {
            id,
            options,
            permissions: _,
          } if id.0.as_str() == "env" => {
            for option in options {
              let Some((key, value)) = option.split_once('=') else {
                continue;
              };
              if let Some(state_name) = value.strip_prefix("facet:") {
                if let Some(val) = resolve_state(state_name) {
                  envs.insert(Ustr::from(key), Ustr::from(val));
                }
              } else if let (Some(variables), Some(variable)) =
                (variables, value.strip_prefix("var:"))
              {
                if let Some(val) = variables.get(variable) {
                  envs.insert(Ustr::from(key), Ustr::from(val.to_string()));
                }
              } else {
                envs.insert(Ustr::from(key), Ustr::from(value));
              }
            }
          }
          TransportMethod::Options {
            id,
            options,
            permissions: _,
          } if id.0.as_str() == "args" => {
            for option in options {
              if let Some(state_name) = option.strip_prefix("facet:") {
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
                args.push(option.clone());
              }
            }
          }
          TransportMethod::Type(id) | TransportMethod::Options { id, .. }
            if id.0.as_str() == "uds" || id.0.as_str() == "shm" =>
          {
            envs.insert(
              Ustr::from("RIND_TP_SOCK"),
              socket_path(&registry_key).to_str().unwrap().to_ustr(),
            );
          }
          TransportMethod::Type(id) if id.0.starts_with("route:") => {
            envs.insert(
              Ustr::from("RIND_TP_SOCK"),
              socket_path(id.0.trim_start_matches("route:"))
                .to_str()
                .unwrap()
                .to_ustr(),
            );
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
      envs.insert(Ustr::from("RIND_SOCKET_FDS"), Ustr::from(inherited_fds));
      envs.insert(
        Ustr::from("RIND_SOCKET_COUNT"),
        Ustr::from(activation_fds.len().to_string()),
      );
      envs.insert(
        Ustr::from("LISTEN_FDS"),
        Ustr::from(activation_fds.len().to_string()),
      );
      if !activation_names.is_empty() {
        let names = activation_names
          .iter()
          .map(|x| x.as_str())
          .collect::<Vec<_>>()
          .join(":");
        envs.insert(Ustr::from("RIND_SOCKET_NAMES"), Ustr::from(names.clone()));
        envs.insert(Ustr::from("LISTEN_FDNAMES"), Ustr::from(names));
      }
    }

    if let Some(watchdog) = &watchdog_cfg {
      envs.insert(
        Ustr::from("RIND_WATCHDOG_GRACE_MS"),
        Ustr::from(watchdog.grace_ms.to_string()),
      );
      if let Some(interval_ms) = watchdog.interval_ms {
        envs.insert(
          Ustr::from("RIND_WATCHDOG_INTERVAL_MS"),
          Ustr::from(interval_ms.to_string()),
        );
      }
    }

    let executor_name = run.executor.clone().unwrap_or_else(|| Ustr::from("native"));
    let executor = self
      .executors
      .get(&executor_name)
      .ok_or_else(|| CoreError::Custom(format!("Executor {} not found", executor_name)))?;

    let mut handle = executor.spawn(ExecutorContext {
      service,
      run,
      log,
      branch_ctx,
      sockets_map,
      sm,
      variables,
      registry_key: registry_key.clone(),
      notifier: notifier.clone(),
      resources,
      resolved_user: resolved_user.clone(),
      envs,
      args,
      isolation: isolation.clone(),
      cgroup_path: Self::cgroup_path_for(
        service,
        isolation.cgroup.as_ref(),
        branch_ctx,
        resolved_user.as_ref(),
      ),
      namespace_mounts,
      namespace_networks,
    })?;

    if let Some(pid) = handle.pid() {
      if let Err(e) = self.setup_cgroup_for_pid(
        service,
        if isolation.needs_namespace_supervisor() {
          None
        } else {
          isolation.cgroup.as_ref()
        },
        branch_ctx,
        resolved_user.as_ref(),
        pid,
      ) {
        let _ = handle.kill(Signal::SIGKILL);
        return Err(CoreError::InvalidState(format!(
          "failed to setup cgroup for service '{}': {e}",
          service.metadata.name
        )));
      }
      self.pid_map.insert(pid, registry_key.clone());

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
    }

    if service
      .metadata
      .transport
      .as_ref()
      .map(is_stdio_transport)
      .unwrap_or(false)
    {
      start_stdout_listener(
        registry_key.clone(),
        handle.take_stdout(),
        self.stdio_tx.clone(),
        notifier,
        service.instances.len(), // TODO: use a better rigid way to handle indexing
      );
      let (tx, rx) = mpsc::channel::<TransportMessage>();
      start_stdin_writer(registry_key.clone(), handle.take_stdin(), log.clone(), rx);
      if let Ok(mut writers) = self.stdio_writers.lock() {
        writers.entry(registry_key.clone()).or_default().push(tx);
      }
    } else {
      start_service_stream_logs(
        registry_key.clone(),
        handle.take_stdout(),
        handle.take_stderr(),
        log.clone(),
      );
    }

    Ok(ChildInstance::new(
      branch_key.cloned().unwrap_or_default(),
      resolved_user,
      Some(handle),
    ))
  }

  pub fn start_service(
    &mut self,
    service: &mut Service,
    log: &LogHandle,
    sockets_map: &HashMap<Ustr, SocketActivation>,
    sm: Option<&FacetGraph>,
    dispatch: &RuntimeDispatcher,
    variable_heap: Option<&VariableHeap>,
    registry_key: Ustr,
    notifier: Option<Notifier>,
    resources: &mut Resources,
    namespace_mounts: Vec<NamespaceMountEntry>,
    namespace_networks: Vec<NamespaceNetworkConfig>,
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
      sockets_map,
      sm,
      variable_heap,
      registry_key.clone(),
      notifier,
      resources,
      namespace_mounts,
      namespace_networks,
    ) {
      Ok(_) => {
        self.register_service_transport(service, dispatch, Some(registry_key.clone()));
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
        service.last_state = ServiceState::Error(err.clone());
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
    sm: Option<&FacetGraph>,
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
    if let Some(handle) = inst.handle.as_mut() {
      let signal = if mode == StopMode::ForceKill {
        Signal::SIGKILL
      } else {
        Signal::SIGTERM
      };
      let _ = handle.kill(signal);
      if let Some(pid) = handle.pid() {
        self.disarm_watchdog_pid(pid, resources);
        self.stopping_map.insert(pid, Instant::now());
      }
      inst.state = ServiceState::Stopping;
      inst.stop_time = Some(Instant::now());
      inst.manually_stopped = true;
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
    sm: Option<&FacetGraph>,
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
        .to_string();
      let full_name = Self::instance_key_name(&full_name);

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
    sm: Option<&FacetGraph>,
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
      inst.handle = None;
      (inst.manually_stopped, inst.retry_count)
    };

    service.last_state = ServiceState::Exited(code);

    self.maybe_unregister_service_transport(service, dispatch, Some(&service_key));

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

      let full_name = Self::instance_key_name(service_key.as_str());

      let _ = SocketRuntime::actions
        .clear_for(full_name.clone())
        .dispatch(dispatch);
      let _ = SocketRuntime::actions
        .resume_fds(full_name)
        .dispatch(dispatch)
        .ok()?;
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
    sm: Option<&FacetGraph>,
    dispatch: &RuntimeDispatcher,
  ) {
    if let Some(triggers) = triggers {
      trigger_events(triggers.clone(), sm, dispatch);
    }
  }

  fn register_service_transport(
    &self,
    service: &Service,
    dispatch: &RuntimeDispatcher,
    registry_key: Option<Ustr>,
  ) -> bool {
    let Some(transport) = &service.metadata.transport else {
      return false;
    };

    let endpoint = registry_key.unwrap_or(service.metadata.name.clone());

    if is_stdio_transport(transport) {
      let _ = dispatch.dispatch(
        "transport",
        "register_stdio",
        rpayload!({ "endpoint": endpoint }),
      );
      false
    } else {
      let id = transport_id(transport);

      if id == "uds" || id == "shm" {
        let action = if id == "uds" {
          "setup_uds"
        } else {
          "setup_shm"
        };
        let payload = rpayload!({ "endpoint": endpoint });
        let _ = dispatch.dispatch(
          "transport",
          action,
          if let Some(perms) = transport.get_permissions() {
            payload.insert("permissions", perms)
          } else {
            payload
          },
        );
        true
      } else if id.starts_with("route:") {
        let _ = TransportRuntime::actions
          .setup_route(id.to_ustr())
          .dispatch(dispatch);
        true
      } else {
        false
      }
    }
  }

  fn maybe_unregister_service_transport(
    &self,
    service: &Service,
    dispatch: &RuntimeDispatcher,
    service_key: Option<&Ustr>,
  ) {
    let Some(transport) = &service.metadata.transport else {
      return;
    };

    if !is_stdio_transport(transport) {
      return;
    }

    let active = service.instances.iter().any(|inst| inst.handle.is_some());
    if active {
      return;
    }

    let _ = TransportRuntime::actions
      .unregister_stdio(
        service_key
          .cloned()
          .unwrap_or(service.metadata.name.clone()),
      )
      .dispatch(dispatch);
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
        rind_core::prelude::FlowEventType::Facet => TransportMessageType::Facet,
        rind_core::prelude::FlowEventType::Impulse => TransportMessageType::Impulse,
      },
      payload: Some(FlowPayload::from_json(Some(event.payload.clone()))),
      branch: None,
      name: Some(event.name.clone()),
      action: if event.action == FlowAction::Revert {
        TransportMessageAction::Remove
      } else {
        TransportMessageAction::Set
      },
    };

    for entries in writers.values_mut() {
      entries.retain(|tx| tx.send(msg.clone()).is_ok());
    }
    writers.retain(|_, entries| !entries.is_empty());
  }

  fn check_branch_match(
    &self,
    spec: &str,
    key: &str,
    sm: &FacetGraph,
    vh: Option<&VariableHeap>,
  ) -> bool {
    let key_val = serde_json::json!(key);

    if let Some(var_name) = spec.strip_prefix("var:") {
      if let Some(vh) = vh {
        if let Some(val) = vh.get(var_name) {
          if let Some(arr) = val.as_array() {
            for val in arr {
              if let Ok(json_val) = serde_json::to_value(val) {
                if subset_match(&key_val, &json_val) {
                  return true;
                }
              }
            }
            return false;
          } else if let Ok(json_val) = serde_json::to_value(val) {
            return subset_match(&key_val, &json_val);
          }
        }
      }
      return false;
    }

    if let Some(state_name) = spec.strip_prefix("facet:") {
      let (state_name, key) = if let Some((s, k)) = state_name.split_once("/") {
        (s, Some(k))
      } else {
        (state_name, None)
      };
      if let Some(instances) = sm.facets.get(&Ustr::from(state_name)) {
        for inst in instances {
          if subset_match(
            &key_val,
            &if let Some(key) = &key {
              inst.payload.get_json_field(key).unwrap_or_default()
            } else {
              inst.payload.to_json()
            },
          ) {
            return true;
          }
        }
      }
    }

    if let Some(var_name) = spec.strip_prefix("many:") {
      for var_name in var_name.split(",") {
        if subset_match(&key_val, &serde_json::json!(var_name.trim())) {
          return true;
        }
      }
    }

    return subset_match(&key_val, &serde_json::json!(spec));
  }

  fn reconcile(
    &mut self,
    ctx: &mut RuntimeContext<'_>,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    name: Ustr,
    action: ServiceEventKind,
  ) -> CoreResult<Void> {
    self.__runtime_reconcile_stacks(
      rpayload!({
        "service": name.clone(),
        "action": action
      }),
      ctx,
      dispatch,
      log,
    )?;

    TimerRuntime::actions
      .reconcile_timers(name, action)
      .dispatch(dispatch)?;

    Ok(Void)
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
    TransportMethod::Object { id, .. } => id.0.as_str() == "stdio",
  }
}

fn start_service_stream_logs(
  service_name: Ustr,
  stdout: Option<Box<dyn std::io::Read + Send>>,
  stderr: Option<Box<dyn std::io::Read + Send>>,
  log: LogHandle,
) {
  if let Some(stdout) = stdout {
    let service_name = service_name.clone();
    let log = log.clone();
    std::thread::spawn(move || {
      let reader = BufReader::new(stdout);
      for line_res in reader.lines() {
        let Ok(line) = line_res else { continue };
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

  if let Some(stderr) = stderr {
    std::thread::spawn(move || {
      let reader = BufReader::new(stderr);
      for line_res in reader.lines() {
        let Ok(line) = line_res else { continue };
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
  stdin: Option<Box<dyn std::io::Write + Send>>,
  log: LogHandle,
  rx: Receiver<TransportMessage>,
) {
  let Some(mut stdin) = stdin else {
    return;
  };

  std::thread::spawn(move || {
    while let Ok(msg) = rx.recv() {
      let payload = ser_to_vec(&msg, true);
      let len = (payload.len() as u32).to_be_bytes();

      if std::io::Write::write_all(&mut stdin, &len).is_err()
        || std::io::Write::write_all(&mut stdin, &payload).is_err()
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceBranchContext {
  pub key: Option<Ustr>,
  pub payload: Option<FlowPayload>,
  pub forced_user: Option<Ustr>,
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

  let svc = ctx.registry.metadata.find::<Service>("*", &payload.name);
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

  let mut dispatch_payload = ServiceRuntime::actions.start(payload.name.to_ustr());

  if let (false, false, Some(username)) = (
    uid == 0,
    pm.user_has(uid, PERM_SYSTEM_SERVICES),
    caller.map(|u| u.username.clone()),
  ) {
    dispatch_payload = dispatch_payload.only_user(username.to_string());
  }

  dispatch_payload.dispatch(dispatch)?;

  if payload.persist {
    FlowRuntime::actions
      .set_facet("rind:active".into())
      .payload(serde_json::Value::String(payload.name.clone()))
      .dispatch(dispatch)?;
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

  let svc = ctx.registry.metadata.find::<Service>("*", &payload.name);
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

  let mut dispatch_payload = ServiceRuntime::actions
    .stop(payload.name.to_ustr())
    .force(payload.force);

  if let (false, false, Some(username)) = (
    uid == 0,
    pm.user_has(uid, PERM_SYSTEM_SERVICES),
    caller.map(|u| u.username.clone()),
  ) {
    dispatch_payload = dispatch_payload.only_user(username);
  }

  dispatch_payload.dispatch(dispatch)?;

  if payload.persist {
    FlowRuntime::actions
      .remove_facet("rind:active".into())
      .payload(serde_json::Value::String(payload.name.clone()))
      .dispatch(dispatch)?;
  }

  Ok(Message::ok(format!("stopped {}", payload.name)))
}

#[runtime("services")]
impl ServiceRuntime {
  fn bootstrap(&mut self) {
    self.rebuild_trigger_index(ctx.registry.metadata);
  }

  fn send_stdio(&mut self, endpoint: String, message: TransportMessage) {
    self.send_stdio_message(endpoint.as_str(), message);
  }

  fn drain_events(&mut self, #[optional] event: FlowEvent) {
    if let Some(w) = event {
      self.broadcast_stdio_event(&w);
    }

    while let Ok((service_name, message, index)) = self.stdio_rx.try_recv() {
      let user = if let Ok(service) = ctx.registry.as_one::<Service>("*", service_name.clone()) {
        if let Some(inst) = service.instances.get(index) {
          inst.user.clone()
        } else {
          self.maybe_unregister_service_transport(service, dispatch, Some(&service_name));
          continue;
        }
      } else {
        continue;
      };

      let uid = if let Some(user) = &user {
        ctx
          .scope
          .get::<PermissionStore>()
          .cloned()
          .unwrap_or_default()
          .users
          .lookup_by_name(&user)
          .map(|u| u.uid)
          .unwrap_or(0)
      } else {
        0
      };

      TransportRuntime::actions
        .ingest(service_name, uid, message)
        .dispatch(dispatch)?;
    }
  }

  fn watchdog_ping(&mut self, service: Ustr, #[optional] branch: Ustr) {
    let fds: Vec<RawFd> = self
      .watchdog_fds
      .iter()
      .filter_map(|(fd, binding)| {
        if binding.service_key != service {
          return None;
        }
        if let Some(branch_val) = &branch
          && binding.branch.as_ref() != Some(branch_val)
        {
          return None;
        }
        Some(*fd)
      })
      .collect();

    if let Some(service_meta) = ctx
      .registry
      .metadata
      .find::<Service>("*", Self::instance_key_name(service.as_str()).as_str())
      && let Some(watchdog) = &service_meta.watchdog
    {
      for fd in fds {
        let _ = self.refresh_watchdog_fd(fd, watchdog);
      }
    }
  }

  fn watchdog_expired(&mut self, fd: i32) {
    let Some(binding) = self.watchdog_fds.get(&(fd as RawFd)).cloned() else {
      return Ok(None);
    };
    let action = ctx
      .registry
      .metadata
      .find::<Service>(
        "*",
        Self::instance_key_name(binding.service_key.as_str()).as_str(),
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
        self.__runtime_stop(
          rpayload!({
            "name": Self::instance_key_name(binding.service_key.as_str()),
            "force": true,
            "only_user": binding.user.clone()
          }),
          ctx,
          dispatch,
          log,
        )?;
      }
      WatchdogAction::Restart => {
        self.__runtime_stop(
          rpayload!({
            "name": Self::instance_key_name(binding.service_key.as_str()),
            "force": true,
            "only_user": binding.user.clone()
          }),
          ctx,
          dispatch,
          log,
        )?;

        self.__runtime_start(
          rpayload!({
            "name": Self::instance_key_name(binding.service_key.as_str()),
            "only_user": binding.user
          }),
          ctx,
          dispatch,
          log,
        )?;
      }
    }
  }

  fn evaluate_triggers(&mut self, #[default] trigger: EmitTrigger, #[optional] scope: Ustr) {
    let scope_val = scope.unwrap_or("static".to_ustr());

    if self.trigger_index.is_empty() {
      self.rebuild_trigger_index(ctx.registry.metadata);
    }

    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), Option<Vec<(Ustr, Option<ServiceBranchContext>)>>>(
        (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
        |registry, (sm, vh)| {
          let target_keys = if let Some(event_name) = trigger.name.as_ref() {
            let mut out = HashSet::new();
            let direct = event_name.clone();
            let static_alias = if event_name.as_str().ends_with("@static") {
              Ustr::from(event_name.as_str().trim_end_matches("@static"))
            } else {
              Ustr::from(format!("{}@static", event_name))
            };
            for key in [direct, static_alias] {
              if let Some(found) = self.trigger_index.get(&key) {
                out.extend(found.iter().cloned());
              }
            }
            out
          } else {
            registry
              .metadata
              .items::<Service>(scope_val.clone())
              .unwrap_or_default()
              .into_iter()
              .map(|(group, meta)| Ustr::from(format!("{}:{}@{}", group, meta.name, scope_val)))
              .collect::<HashSet<Ustr>>()
          };

          let emit_event = match (
            trigger.name.as_ref(),
            trigger.flow_type,
            trigger.payload.as_ref(),
          ) {
            (Some(name), Some(flow_type), Some(payload)) => Some(FlowInstance {
              name: name.clone().into(),
              payload: payload.clone(),
              r#type: flow_type,
            }),
            _ => None,
          };

          let mut to_start = Vec::new();

          for service_name in target_keys {
            let mut is_running = false;

            let Some(meta) = registry
              .metadata
              .find::<Service>("*", service_name.as_str())
            else {
              continue;
            };

            let Some((_unit, _)) = service_name.split_once(':') else {
              continue;
            };

            let service_key = Self::ensure_scoped_name(service_name.as_str());

            if let Some(instances) = registry.instances.get_mut(&service_key) {
              for instance in instances.iter_mut() {
                if let Some(service) = instance.downcast_mut::<Service>() {
                  is_running = service
                    .instances
                    .iter()
                    .any(|i| i.state == ServiceState::Active || i.state == ServiceState::Starting);

                  if let Some(ref branching) = service.metadata.branching {
                    match (trigger.action, emit_event.as_ref(), is_running) {
                      (FlowAction::Revert, Some(event), true)
                        if branching.key.is_some() && event.name == branching.source =>
                      {
                        let key =
                          Self::branch_key_from_payload(&event.payload, branching.key.as_deref());

                        let to_stop: Vec<Ustr> = service
                          .instances
                          .iter()
                          .filter_map(|inst| {
                            if (inst.state == ServiceState::Active
                              || inst.state == ServiceState::Starting)
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
                        conds
                          .iter()
                          .any(|cond| condition_matches(sm, cond, emit_event.as_ref(), None))
                      })
                      .unwrap_or(false);

                    let auto_stop_on_revert = if service.metadata.stop_on.is_none() {
                      match (
                        trigger.action,
                        emit_event.as_ref(),
                        service.metadata.start_on.as_ref(),
                      ) {
                        (FlowAction::Revert, Some(event), Some(start_conds)) => {
                          start_conds.iter().any(|cond| {
                            check_condition(cond, event) && !condition_is_active(sm, cond, None)
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
                conds
                  .iter()
                  .all(|cond| condition_matches(sm, cond, emit_event.as_ref(), None))
              })
              .unwrap_or(false);

            if !should_start {
              continue;
            }

            if meta.branching.is_none() && is_running {
              continue;
            }

            let (service_scope, service_base_name) =
              if let Some((base, scope)) = service_name.rsplit_once('@') {
                (Ustr::from(scope), Ustr::from(base))
              } else {
                (Ustr::from("static"), service_name.clone())
              };
            let ser = registry
              .instantiate_one(service_scope, service_base_name, |x| Ok(Service::new(x)))?;

            if let Some(branching) = &ser.metadata.branching {
              let mut branches = sm
                .facets
                .get(&branching.source)
                .cloned()
                .unwrap_or_default();

              if let Some(event) = emit_event.as_ref() {
                if event.r#type == FlowType::Impulse && event.name == branching.source {
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
                to_start.push((service_name.clone(), Some(branch_ctx)));
                started += 1;
              }
              continue;
            }

            to_start.push((service_name.clone(), None));
          }
          Ok(if to_start.is_empty() {
            None
          } else {
            Some(to_start)
          })
        },
      )?.map(|to_start| {
        for (service_name, branch_ctx) in to_start {
          let mut payload = rpayload!({ "name": service_name });

          if let Some(branch_ctx) = branch_ctx {
            payload = payload.insert("branch_ctx", branch_ctx);
          }

          self.__runtime_start(payload, ctx, dispatch, log)?;
        }

        Ok::<Void, CoreError>(Void)
      }).transpose()?;
  }

  fn start(
    &mut self,
    name: Ustr,
    #[default] socket_fds: Vec<i32>,
    #[default] socket_fd_names: Vec<Ustr>,
    #[optional] only_user: String,
    #[optional] branch_ctx: ServiceBranchContext,
    #[default] deferred: bool,
  ) {
    let socket_fds_raw = socket_fds.iter().map(|fd| *fd as RawFd).collect::<Vec<_>>();
    let mut sockets_map = get_all_sockets(&ctx.registry);
    if !socket_fds_raw.is_empty() {
      let entry = sockets_map
        .entry(name.clone())
        .or_insert_with(|| SocketActivation {
          fds: Vec::new(),
          names: Vec::new(),
        });
      entry.fds.extend(socket_fds_raw.clone());
      entry.names.extend(socket_fd_names.clone());
    }

    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), Option<(
        Ustr,
        Vec<i32>,
        Vec<Ustr>,
        bool,
        Option<String>,
        Option<ServiceBranchContext>,
      )>>(
        (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
        |registry, (sm, vh)| {
          let service_key = Self::ensure_scoped_name(name.as_str());
          let metadata_registry = registry.metadata;
          let service =
            registry.instantiate_one::<Service>("*", name.clone(), |x| Ok(Service::new(x)))?;

          let ns_scope = name.rsplit_once('@').map(|(_, s)| s).unwrap_or("static");
          let ns_mounts: Vec<NamespaceMountEntry> = metadata_registry
            .items::<Mount>(ns_scope)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, m)| NamespaceMountEntry {
              source: m.source.as_ref().map(|s| s.to_string()),
              target: m.target.to_string(),
              fstype: m.fstype.as_ref().map(|s| s.to_string()),
              flags: m.flags.as_ref().map(|f| f.clone()),
              data: m.data.clone(),
              create: m.create,
            })
            .collect();
          let ns_networks: Vec<NamespaceNetworkConfig> = Vec::new();

          if !deferred
            && self.register_service_transport(service, dispatch, Some(service_key.clone()))
          {
            return Ok(Some((
              name,
              socket_fds,
              socket_fd_names,
              true,
              only_user,
              branch_ctx,
            )));
          }

          // because async
          if service
            .metadata
            .transport
            .as_ref()
            .map(is_stdio_transport)
            .unwrap_or(false)
          {
            TransportRuntime::actions
              .register_stdio(name)
              .dispatch(dispatch)?;
          }

          if let Some(branch_ctx_val) = branch_ctx.clone() {
            if let Some(ref key) = branch_ctx_val.key
              && service.instances.iter().any(|i| {
                i.key == *key
                  && (i.state == ServiceState::Active || i.state == ServiceState::Starting)
              })
            {
              return Ok(None);
            }

            match self.spawn_all(
              service,
              log,
              Some(&branch_ctx_val),
              &sockets_map,
              Some(sm),
              Some(vh),
              service_key.clone(),
              ctx.notifier.clone(),
              ctx.resources,
              ns_mounts.clone(),
              ns_networks.clone(),
            ) {
              Ok(instances) => {
                service.instances.extend(instances);
                self.register_service_transport(service, dispatch, Some(service_key.clone()));
                if !service.instances.is_empty() {
                  for inst in service.instances.iter_mut() {
                    inst.state = ServiceState::Active;
                  }
                  self.run_triggers(service.metadata.on_start.as_ref(), Some(sm), dispatch);
                }

                Ok(Some((
                  service_key,
                  socket_fds,
                  socket_fd_names,
                  false,
                  only_user,
                  branch_ctx,
                )))
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
                for inst in service.instances.iter_mut() {
                  if inst.state != ServiceState::Active {
                    inst.state = ServiceState::Error(err.clone());
                  }
                }
                Ok(None)
              }
            }
          } else if let Some(user) = only_user.clone() {
            let launch_ctx = ServiceBranchContext {
              key: None,
              payload: None,
              forced_user: Some(user.into()),
            };

            match self.spawn_all(
              service,
              log,
              Some(&launch_ctx),
              &sockets_map,
              Some(sm),
              Some(vh),
              service_key.clone(),
              ctx.notifier.clone(),
              ctx.resources,
              ns_mounts.clone(),
              ns_networks.clone(),
            ) {
              Ok(instances) => {
                service.instances.extend(instances);
                self.register_service_transport(service, dispatch, Some(service_key.clone()));
                if let Some(inst) = service.instances.as_one_mut() {
                  inst.state = ServiceState::Active;
                  self.run_triggers(service.metadata.on_start.as_ref(), Some(sm), dispatch);
                }

                Ok(Some((
                  service_key,
                  socket_fds,
                  socket_fd_names,
                  false,
                  only_user,
                  Some(launch_ctx),
                )))
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
                Ok(None)
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
              service_key.clone().into(),
              ctx.notifier.clone(),
              ctx.resources,
              ns_mounts,
              ns_networks,
            );

            Ok(Some((
              service_key,
              socket_fds,
              socket_fd_names,
              false,
              only_user,
              branch_ctx,
            )))
          }
        },
      )?
      .map(
        |(name, socket_fds, socket_fd_names, deferred, only_user, branch_ctx)| {
          if deferred {
            let mut payload = rpayload!({
              "name": name,
              "socket_fds": socket_fds,
              "socket_fd_names": socket_fd_names,
              "deferred": true
            });
            if let Some(user) = only_user {
              payload = payload.insert("only_user", user);
            }
            if let Some(bctx) = branch_ctx {
              payload = payload.insert("branch_ctx", bctx);
            }

            // self.__runtime_start(payload, ctx, dispatch, log)?;
            dispatch.dispatch("services", "start", payload)?;
          } else {
            self.reconcile(ctx, log, dispatch, name, ServiceEventKind::Started)?;
          }

          Ok::<Void, CoreError>(Void)
        },
      )
      .transpose()?;
  }

  fn stop(
    &mut self,
    name: Ustr,
    #[default] force: bool,
    #[optional] index: usize,
    #[optional] only_user: Ustr,
  ) {
    let mode = if force {
      StopMode::ForceKill
    } else {
      StopMode::Graceful
    };
    let notifier = ctx.notifier.clone();

    ctx.registry.singleton_handle::<(&mut FacetGraph,), _>(
      (FacetGraph::KEY.into(),),
      |registry, (sm,)| {
        let service =
          registry.instantiate_one::<Service>("*", name.clone(), |x| Ok(Service::new(x)))?;

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
        Ok(Void)
      },
    )?;
  }

  fn stop_all(&mut self, #[default] force: bool) {
    let mode = if force {
      StopMode::ForceKill
    } else {
      StopMode::Graceful
    };
    let notifier = ctx.notifier.clone();

    ctx.registry.singleton_handle::<(&mut FacetGraph,), _>(
      (FacetGraph::KEY.into(),),
      |registry, (sm,)| {
        let keys: Vec<Ustr> = registry
          .instances
          .keys()
          .filter(|k| k.contains('@'))
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

        Ok(Void)
      },
    )?;
  }

  fn start_all(&mut self) {
    let mut started: HashSet<Ustr> = HashSet::new();
    let mut pending: Vec<(Ustr, Vec<Ustr>, Arc<ServiceMetadata>)> = Vec::new();

    for (full_name, svc_meta) in &ctx
      .registry
      .singleton_handle::<(&mut FacetGraph,), Vec<(Ustr, Arc<ServiceMetadata>)>>(
        (FacetGraph::KEY.into(),),
        |_, (sm,)| {
          let mut all_services: Vec<(Ustr, Arc<ServiceMetadata>)> = Vec::new();

          let Some(active) = sm.facets.get("rind:active") else {
            return Ok(all_services);
          };

          for branch in active {
            let name = Ustr::from(branch.payload.to_string_payload());
            if let Some(svc) = ctx.registry.metadata.find::<Service>("*", name.as_str()) {
              all_services.push((name, svc));
            }
          }

          Ok(all_services)
        },
      )?
    {
      if let Some(afters) = &svc_meta.after {
        pending.push((full_name.clone(), afters.clone(), svc_meta.clone()));
      } else {
        let key = Self::ensure_scoped_name(full_name.as_str());
        let already_running = ctx
          .registry
          .as_one::<Service>("*", key.clone())
          .ok()
          .map(|svc| {
            svc.instances.iter().any(|inst| {
              inst.state == ServiceState::Active || inst.state == ServiceState::Starting
            })
          })
          .unwrap_or(false);

        if !already_running {
          self.__runtime_start(rpayload!({ "name": full_name.clone() }), ctx, dispatch, log)?;
        }
        started.insert(full_name.clone());
      }
    }

    loop {
      let mut progress = false;
      pending.retain(|(name, afters, _meta)| {
        if afters.iter().all(|a| started.contains(a)) {
          let key = Self::ensure_scoped_name(name.as_str());
          let already_running = ctx
            .registry
            .as_one::<Service>("*", key.clone())
            .ok()
            .map(|svc| {
              svc.instances.iter().any(|inst| {
                inst.state == ServiceState::Active || inst.state == ServiceState::Starting
              })
            })
            .unwrap_or(false);

          if !already_running {
            let _ = self.__runtime_start(rpayload!({ "name": name.clone() }), ctx, dispatch, log);
          }
          started.insert(name.clone());
          progress = true;
          false
        } else {
          true
        }
      });
      if !progress {
        break;
      }
    }

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

  fn reconcile_stacks(&mut self, service: Ustr, action: ServiceEventKind) {
    let service_name = Self::instance_key_name(service.as_str());
    let notifier = ctx.notifier.clone();

    let mut dependents: Vec<(Ustr, Arc<ServiceMetadata>)> = Vec::new();
    for meta_name in ctx.registry.metadata.metadata_names() {
      let Some(meta) = ctx.registry.metadata.metadata(meta_name.clone()) else {
        continue;
      };
      for group in meta.groups() {
        if let Some(svcs) = ctx
          .registry
          .metadata
          .group_items::<Service>(meta_name.clone(), group.clone())
        {
          for svc in svcs {
            if let Some(ref dependencies) = svc.after
              && dependencies.contains(&service_name)
            {
              dependents.push((Ustr::from(format!("{group}:{}", svc.name)), svc));
            }
          }
        }
      }
    }

    match action {
      ServiceEventKind::Failed
      | ServiceEventKind::Stopped
      | ServiceEventKind::Exited { code: _ } => {
        ctx
          .registry
          .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), _>(
            (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
            |registry, (sm, _)| {
              for (dependent, _) in dependents {
                if let Ok(service) = registry.as_one_mut::<Service>("*", dependent.as_str()) {
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
              Ok(Void)
            },
          )?;
      }
      ServiceEventKind::Started => {
        for (dependent, svc) in dependents {
          let should_start = svc.after.as_ref().unwrap().iter().any(|a| {
            if let Ok(ref svc) = ctx.registry.as_one::<Service>("*", a.as_str()) {
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
            self.__runtime_start(rpayload!({ "name": dependent.clone() }), ctx, dispatch, log)?;
          }
        }
      }
    }
  }

  fn stop_for_scope(&mut self, scope: Ustr) {
    let notifier = ctx.notifier.clone();

    ctx.registry.singleton_handle::<(&mut FacetGraph,), _>(
      (FacetGraph::KEY.into(),),
      |registry, (sm,)| {
        for (group, svc) in registry
          .metadata
          .items::<Service>(scope.clone())
          .unwrap_or_default()
        {
          let full_name = rslvns!(u group, svc.name);
          self.stop_service(
            registry.as_one_mut::<Service>(scope.clone(), full_name.clone())?,
            StopMode::ForceKill,
            log,
            dispatch,
            Some(sm),
            None,
            None,
            None,
            Some(&full_name),
            notifier.clone(),
            ctx.resources,
          );
        }
        Ok(Void)
      },
    )?;
  }

  fn child_exited(&mut self, pid: i32, code: i32) {
    let pid_u = pid as u32;
    if let Some(service_key) = self.pid_map.remove(&pid_u) {
      self.stopping_map.remove(&pid_u);

      match ctx
        .registry
        .singleton_handle::<(&mut FacetGraph,), Option<(Ustr, ServiceEventKind)>>(
          (FacetGraph::KEY.into(),),
          |registry, (sm,)| {
            let mut action = None;
            if let Some(instances) = registry.instances.get_mut(&service_key) {
              for instance in instances.iter_mut() {
                if let Some(service) = instance.downcast_mut::<Service>() {
                  if let Some(exit_action) = self.handle_child_exit(
                    service,
                    pid,
                    code,
                    log,
                    dispatch,
                    Some(sm),
                    service_key.clone(),
                    ctx.resources,
                  ) {
                    match exit_action {
                      ServiceExitAction::Restart => {
                        action = Some((service_key.clone(), ServiceEventKind::Started));
                      }
                      ServiceExitAction::StopDependents => {
                        action = Some((service_key.clone(), ServiceEventKind::Exited { code }));
                      }
                    }
                  }
                }
              }
            }
            Ok(action)
          },
        )? {
        Some((service, action)) => {
          if action == ServiceEventKind::Started {
            self.__runtime_start(
              rpayload!({
                "name": service
              }),
              ctx,
              dispatch,
              log,
            )?;
          } else {
            self.reconcile(ctx, log, dispatch, service, action)?;
          }
        }
        None => {}
      }
    }
  }

  fn timeout_sweep(&mut self) {
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
}

#[cfg(test)]
mod tests {
  use super::*;
  use rind_core::prelude::Metadata;
  use rind_ipc::FlowJson;

  fn service_from_toml(source: &str) -> Service {
    let mut metadata = Metadata::new("test").of::<Service>("service");
    metadata.from_toml(source, "svc").unwrap();
    let service = metadata
      .get_in_group::<Service>("svc")
      .and_then(|services| services.first())
      .cloned()
      .expect("service should parse");
    Service::new(service)
  }

  #[test]
  fn isolation_uses_scope_cgroup_defaults_and_service_overrides() {
    let scope = "iso_cgroup_test";
    let mut attrs = HashMap::new();
    attrs.insert(Ustr::from("cgroup.path"), "rind/scope-default".to_string());
    attrs.insert(Ustr::from("cgroup.memory-max"), "128M".to_string());
    attrs.insert(Ustr::from("cgroup.pids-max"), "64".to_string());
    ScopeStore::upsert_global(scope, attrs, None);

    let service = service_from_toml(
      r#"
[[service]]
name = "demo"
run.exec = "/bin/true"
cgroup = { path = "rind/service", cpu-max = "50000 100000" }
"#,
    );

    let isolation = ServiceRuntime::isolation_for(&service, Some(scope)).unwrap();
    let cgroup = isolation.cgroup.expect("cgroup should merge");

    assert_eq!(cgroup.path.unwrap().as_str(), "rind/service");
    assert_eq!(cgroup.memory_max.unwrap().as_str(), "128M");
    assert_eq!(cgroup.cpu_max.unwrap().as_str(), "50000 100000");
    assert_eq!(cgroup.pids_max.unwrap().as_str(), "64");

    ScopeStore::remove_scope_global(scope);
  }

  #[test]
  fn isolation_merges_scope_namespace_attributes_with_service_config() {
    let scope = "iso_namespace_test";
    let mut attrs = HashMap::new();
    attrs.insert(Ustr::from("namespace.mount"), "true".to_string());
    attrs.insert(Ustr::from("namespace.rootfs"), "/scope-root".to_string());
    attrs.insert(Ustr::from("namespace.hostname"), "scope-host".to_string());
    ScopeStore::upsert_global(scope, attrs, None);

    let service = service_from_toml(
      r#"
[[service]]
name = "demo"
run.exec = "/bin/true"
namespaces = { ipc = true, hostname = "service-host" }
"#,
    );

    let isolation = ServiceRuntime::isolation_for(&service, Some(scope)).unwrap();
    let namespaces = isolation.namespaces.expect("namespaces should merge");

    assert!(namespaces.mount);
    assert!(namespaces.ipc);
    assert_eq!(namespaces.rootfs.unwrap().as_str(), "/scope-root");
    assert_eq!(namespaces.hostname.unwrap().as_str(), "service-host");

    ScopeStore::remove_scope_global(scope);
  }

  #[test]
  fn inline_service_namespace_rejects_scope_only_features() {
    let service = service_from_toml(
      r#"
[[service]]
name = "demo"
run.exec = "/bin/true"
namespaces = { pid = true, user = true, persist = true, init = true }
"#,
    );

    let err = ServiceRuntime::isolation_for(&service, Some("static")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("scope-only namespace feature"));
    assert!(msg.contains("pid"));
    assert!(msg.contains("user"));
    assert!(msg.contains("persist"));
    assert!(msg.contains("init"));
  }

  #[test]
  fn scope_attrs_parse_namespace_security_policy() {
    let scope = "iso_security_test";
    let mut attrs = HashMap::new();
    attrs.insert(Ustr::from("namespace.pid"), "true".to_string());
    attrs.insert(Ustr::from("namespace.user"), "true".to_string());
    attrs.insert(Ustr::from("namespace.persist"), "true".to_string());
    attrs.insert(Ustr::from("namespace.init"), "true".to_string());
    attrs.insert(Ustr::from("capabilities.drop"), "all".to_string());
    attrs.insert(
      Ustr::from("capabilities.keep"),
      "net_bind_service,sys_chroot".to_string(),
    );
    attrs.insert(Ustr::from("seccomp.profile"), "default".to_string());
    ScopeStore::upsert_global(scope, attrs, None);

    let service = service_from_toml(
      r#"
[[service]]
name = "demo"
run.exec = "/bin/true"
"#,
    );

    let isolation = ServiceRuntime::isolation_for(&service, Some(scope)).unwrap();
    let namespaces = isolation.namespaces.expect("scope namespace policy");
    assert!(namespaces.pid);
    assert!(namespaces.user);
    assert!(namespaces.persist);
    assert!(namespaces.init);

    let caps = isolation.capabilities.expect("scope capability policy");
    assert_eq!(caps.drop, vec![Ustr::from("all")]);
    assert_eq!(
      caps.keep,
      vec![Ustr::from("net_bind_service"), Ustr::from("sys_chroot")]
    );
    assert_eq!(
      isolation.seccomp.unwrap().profile.unwrap().as_str(),
      "default"
    );

    ScopeStore::remove_scope_global(scope);
  }

  fn test_facet_graph() -> FacetGraph {
    let dir = std::env::temp_dir().join(format!(
      "rind-fg-test-{}-{}",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    FacetGraph::from_persistence(StatePersistence::new(dir))
  }

  fn test_variable_heap() -> (VariableHeap, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
      "rind-vh-test-{}-{}.toml",
      std::process::id(),
      std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
    ));
    (VariableHeap::new(&path), path)
  }

  #[test]
  fn branch_match_plain_string_exact() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    assert!(rt.check_branch_match("foo", "foo", &sm, None));
  }

  #[test]
  fn branch_match_plain_string_no_match() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    assert!(!rt.check_branch_match("foo", "bar", &sm, None));
  }

  #[test]
  fn branch_match_many_spec_hits() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    assert!(rt.check_branch_match("many:foo,bar,baz", "bar", &sm, None));
    assert!(rt.check_branch_match("many:foo,bar,baz", "foo", &sm, None));
  }

  #[test]
  fn branch_match_many_spec_misses() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    assert!(!rt.check_branch_match("many:foo,bar,baz", "qux", &sm, None));
  }

  #[test]
  fn branch_match_var_spec_string_value() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    let (mut vh, _p) = test_variable_heap();
    vh.set("my_var", toml::Value::String("hello".to_string()));
    assert!(rt.check_branch_match("var:my_var", "hello", &sm, Some(&vh)));
    assert!(!rt.check_branch_match("var:my_var", "world", &sm, Some(&vh)));
  }

  #[test]
  fn branch_match_var_spec_array_value() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    let (mut vh, _p) = test_variable_heap();
    vh.set(
      "my_var",
      toml::Value::Array(vec![
        toml::Value::String("alpha".to_string()),
        toml::Value::String("beta".to_string()),
      ]),
    );
    assert!(rt.check_branch_match("var:my_var", "alpha", &sm, Some(&vh)));
    assert!(rt.check_branch_match("var:my_var", "beta", &sm, Some(&vh)));
    assert!(!rt.check_branch_match("var:my_var", "gamma", &sm, Some(&vh)));
  }

  #[test]
  fn branch_match_var_spec_missing_variable() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    let (vh, _p) = test_variable_heap();
    assert!(!rt.check_branch_match("var:nonexistent", "foo", &sm, Some(&vh)));
  }

  #[test]
  fn branch_match_facet_spec_string_payload() {
    let rt = ServiceRuntime::default();
    let mut sm = test_facet_graph();
    sm.facets.insert(
      Ustr::from("test:state"),
      vec![FlowInstance {
        name: Ustr::from("test:state"),
        payload: FlowPayload::String("target_value".to_string()),
        r#type: FlowType::Facet,
      }],
    );
    assert!(rt.check_branch_match("facet:test:state", "target_value", &sm, None));
    assert!(!rt.check_branch_match("facet:test:state", "wrong_value", &sm, None));
  }

  #[test]
  fn branch_match_facet_spec_json_payload_with_key() {
    let rt = ServiceRuntime::default();
    let mut sm = test_facet_graph();
    sm.facets.insert(
      Ustr::from("test:session"),
      vec![FlowInstance {
        name: Ustr::from("test:session"),
        payload: FlowPayload::Json(FlowJson(r#"{"seat":"0","user":"makano"}"#.to_string())),
        r#type: FlowType::Facet,
      }],
    );
    assert!(rt.check_branch_match("facet:test:session/seat", "0", &sm, None));
    assert!(!rt.check_branch_match("facet:test:session/seat", "1", &sm, None));
  }

  #[test]
  fn branch_match_facet_spec_missing_state() {
    let rt = ServiceRuntime::default();
    let sm = test_facet_graph();
    assert!(!rt.check_branch_match("facet:nonexistent", "foo", &sm, None));
  }

  #[test]
  fn branch_only_filters_unmatched_keys() {
    let branching = BranchingConfig {
      source: Ustr::from("test:base"),
      key: Some("id".to_string()),
      max_instances: None,
      only: Some(vec!["allowed".to_string()]),
      except: None,
    };

    let matched = |key: &str| -> bool {
      let rt = ServiceRuntime::default();
      let sm = test_facet_graph();
      if let Some(onlys) = &branching.only {
        for spec in onlys {
          if rt.check_branch_match(spec, key, &sm, None) {
            return true;
          }
        }
      }
      false
    };

    assert!(matched("allowed"));
    assert!(!matched("denied"));
    assert!(!matched("anything"));
  }

  #[test]
  fn branch_except_skips_matched_keys() {
    let branching = BranchingConfig {
      source: Ustr::from("test:base"),
      key: Some("id".to_string()),
      max_instances: None,
      only: None,
      except: Some(vec!["blocked".to_string()]),
    };

    let should_start = |key: &str| -> bool {
      let rt = ServiceRuntime::default();
      let sm = test_facet_graph();
      if let Some(excepts) = &branching.except {
        for spec in excepts {
          if rt.check_branch_match(spec, key, &sm, None) {
            return false;
          }
        }
      }
      true
    };

    assert!(should_start("allowed"));
    assert!(should_start("anything"));
    assert!(!should_start("blocked"));
  }

  #[test]
  fn branch_only_and_except_combined() {
    let branching = BranchingConfig {
      source: Ustr::from("test:base"),
      key: Some("id".to_string()),
      max_instances: None,
      only: Some(vec!["group_a".to_string(), "group_b".to_string()]),
      except: Some(vec!["group_b".to_string()]),
    };

    let should_start = |key: &str| -> bool {
      let rt = ServiceRuntime::default();
      let sm = test_facet_graph();
      if let Some(onlys) = &branching.only {
        let mut matched = false;
        for spec in onlys {
          if rt.check_branch_match(spec, key, &sm, None) {
            matched = true;
            break;
          }
        }
        if !matched {
          return false;
        }
      }
      if let Some(excepts) = &branching.except {
        for spec in excepts {
          if rt.check_branch_match(spec, key, &sm, None) {
            return false;
          }
        }
      }
      true
    };

    assert!(should_start("group_a"));
    assert!(!should_start("group_b"));
    assert!(!should_start("group_c"));
  }

  #[test]
  fn branch_only_with_many_spec() {
    let branching = BranchingConfig {
      source: Ustr::from("test:base"),
      key: Some("id".to_string()),
      max_instances: None,
      only: Some(vec!["many:seat0,seat1".to_string()]),
      except: None,
    };

    let matched = |key: &str| -> bool {
      let rt = ServiceRuntime::default();
      let sm = test_facet_graph();
      if let Some(onlys) = &branching.only {
        for spec in onlys {
          if rt.check_branch_match(spec, key, &sm, None) {
            return true;
          }
        }
      }
      false
    };

    assert!(matched("seat0"));
    assert!(matched("seat1"));
    assert!(!matched("seat2"));
  }

  #[test]
  fn branch_except_with_facet_spec() {
    let rt = ServiceRuntime::default();
    let mut sm = test_facet_graph();
    sm.facets.insert(
      Ustr::from("test:tty"),
      vec![FlowInstance {
        name: Ustr::from("test:tty"),
        payload: FlowPayload::Json(FlowJson(r#"{"taken":"tty1"}"#.to_string())),
        r#type: FlowType::Facet,
      }],
    );

    let should_start = |key: &str| -> bool {
      let specs = vec!["facet:test:tty/taken".to_string()];
      for spec in &specs {
        if rt.check_branch_match(spec, key, &sm, None) {
          return false;
        }
      }
      true
    };

    assert!(!should_start("tty1"));
    assert!(should_start("tty2"));
  }

  #[test]
  fn branch_key_from_payload_string_variant() {
    let payload = FlowPayload::String("seat0".to_string());
    let key = ServiceRuntime::branch_key_from_payload(&payload, None);
    assert_eq!(key, Some(Ustr::from("seat0")));
  }

  #[test]
  fn branch_key_from_payload_string_empty_is_none() {
    let payload = FlowPayload::String("".to_string());
    let key = ServiceRuntime::branch_key_from_payload(&payload, None);
    assert!(key.is_none());
  }

  #[test]
  fn branch_key_from_payload_none_variant_is_none() {
    let payload = FlowPayload::None(false);
    let key = ServiceRuntime::branch_key_from_payload(&payload, None);
    assert!(key.is_none());
  }

  #[test]
  fn branch_key_from_payload_json_with_key_name() {
    let payload = FlowPayload::Json(FlowJson(r#"{"seat":"tty1","user":"makano"}"#.to_string()));
    let key = ServiceRuntime::branch_key_from_payload(&payload, Some("seat"));
    assert_eq!(key, Some(Ustr::from("tty1")));
  }

  #[test]
  fn branch_key_from_payload_json_missing_key_is_none() {
    let payload = FlowPayload::Json(FlowJson(r#"{"user":"makano"}"#.to_string()));
    let key = ServiceRuntime::branch_key_from_payload(&payload, Some("seat"));
    assert!(key.is_none());
  }

  #[test]
  fn branch_key_from_payload_json_empty_field_is_none() {
    let payload = FlowPayload::Json(FlowJson(r#"{"seat":""}"#.to_string()));
    let key = ServiceRuntime::branch_key_from_payload(&payload, Some("seat"));
    assert!(key.is_none());
  }

  #[test]
  fn branch_max_instances_caps_branch_creation() {
    let branching = BranchingConfig {
      source: Ustr::from("test:base"),
      key: Some("id".to_string()),
      max_instances: Some(2),
      only: None,
      except: None,
    };
    assert_eq!(branching.max_instances, Some(2));
  }

  #[test]
  fn branch_only_and_except_order_matters() {
    let branching = BranchingConfig {
      source: Ustr::from("test:base"),
      key: Some("id".to_string()),
      max_instances: None,
      only: Some(vec!["a".to_string(), "b".to_string()]),
      except: Some(vec!["b".to_string()]),
    };

    let matched = |key: &str| -> bool {
      let rt = ServiceRuntime::default();
      let sm = test_facet_graph();
      if let Some(onlys) = &branching.only {
        let mut found = false;
        for spec in onlys {
          if rt.check_branch_match(spec, key, &sm, None) {
            found = true;
            break;
          }
        }
        if !found {
          return false;
        }
      }
      if let Some(excepts) = &branching.except {
        for spec in excepts {
          if rt.check_branch_match(spec, key, &sm, None) {
            return false;
          }
        }
      }
      true
    };

    assert!(matched("a"));
    assert!(!matched("b"));
    assert!(!matched("c"));
  }
}
