use nix::sys::signal::{Signal, kill};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use rind_core::prelude::*;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RunOption {
  exec: String,
  args: Vec<String>,
  env: Option<HashMap<String, String>>,
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
  pub child: Option<Child>,
  pub state: ServiceState,
  pub retry_count: u32,
}

impl ChildInstance {
  pub fn new(key: String, child: Option<Child>) -> Self {
    Self {
      key,
      child,
      state: ServiceState::Active,
      retry_count: 0,
    }
  }
}

#[derive(Default)]
pub struct ChildInstanceGroup(pub Vec<ChildInstance>);

impl ChildInstanceGroup {
  pub fn as_one(&self) -> Option<&ChildInstance> {
    if self.0.len() > 0 {
      self.0.first()
    } else {
      None
    }
  }

  pub fn as_one_mut(&mut self) -> Option<&mut ChildInstance> {
    if self.0.len() > 0 {
      self.0.first_mut()
    } else {
      None
    }
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

#[model(meta_name = name, meta_fields(name, run, after, branching, restart), derive_metadata(Debug))]
pub struct Service {
  // Metadata
  pub name: String,
  pub run: RunOptions,
  pub after: Option<Vec<String>>,

  // #[serde(rename = "start-on")]
  // pub start_on: Option<Vec<FlowItem>>,
  // #[serde(rename = "stop-on")]
  // pub stop_on: Option<Vec<FlowItem>>,
  // #[serde(rename = "on-start")]
  // pub on_start: Option<Vec<Trigger>>,
  // #[serde(rename = "on-stop")]
  // pub on_stop: Option<Vec<Trigger>>,
  // #[serde(rename = "transport")]
  // pub transport: Option<TransportMethod>,
  pub branching: Option<BranchingConfig>,
  pub restart: Option<RestartPolicy>,

  // Instance data
  pub instances: ChildInstanceGroup,
}

impl Service {
  pub fn new(metadata: Arc<ServiceMetadata>) -> Self {
    Self {
      metadata,
      instances: ChildInstanceGroup::default(),
    }
  }
}

#[derive(Default)]
pub struct ServiceRuntime;

impl ServiceRuntime {
  pub fn spawn_all(
    &self,
    service: &Service,
    branch_key: Option<&str>,
  ) -> anyhow::Result<Vec<ChildInstance>> {
    service
      .metadata
      .run
      .as_many()
      .map(|run| self.spawn_process(run, branch_key))
      .collect()
  }

  pub fn spawn_service(&self, service: &mut Service, log: &LogHandle) -> anyhow::Result<()> {
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service started",
      self.log_fields(service, "start"),
    );

    let instances = self.spawn_all(service, None)?;
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
    run: &RunOption,
    branch_key: Option<&str>,
  ) -> anyhow::Result<ChildInstance> {
    let child = unsafe {
      let mut cmd = Command::new(&run.exec);
      cmd
        .args(&run.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .pre_exec(|| {
          libc::setsid();
          Ok(())
        });
      if let Some(key) = branch_key {
        cmd.env("RIND_BRANCH_KEY", key);
      }
      if let Some(env) = &run.env {
        cmd.envs(env);
      }
      cmd.spawn()?
    };

    Ok(ChildInstance::new(
      branch_key.map(|x| x.to_string()).unwrap_or(String::new()),
      Some(child),
    ))
  }

  pub fn start_service(&self, service: &mut Service, log: &LogHandle) {
    // init_service_transport(service, TransportInitStage/::ServicePreStart);
    match self.spawn_service(service, log) {
      Ok(_) => {
        // init_service_transport(service, TransportInitStage::ServicePostStart);
        service.instances.as_one_mut().unwrap().state = ServiceState::Active;
        println!("yoo");
      }
      Err(e) => {
        let err = format!("Failed to start service \"{}\": {e}", service.metadata.name);
        let mut fields = self.log_fields(service, "start");
        fields.insert("error".into(), e.to_string());
        println!("{err}");
        log.log(
          LogLevel::Error,
          "service-runtime",
          "failed to start service",
          fields,
        );
        service.instances.as_one_mut().unwrap().state = ServiceState::Error(err);
      }
    }
  }
}

impl Runtime for ServiceRuntime {
  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    match action {
      "start" => {
        let name = payload.get::<String>("name")?;
        println!("{:?}", name);

        let service = ctx
          .registry
          .instantiate_one::<Service>("units", &name, |x| Ok(Service::new(x)))?;

        self.start_service(service, log);
      }
      _ => {}
    }
    Ok(())
  }

  fn id(&self) -> &str {
    "services"
  }
}
