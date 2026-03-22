use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rind_core::prelude::*;

use crate::flow::{FlowInstance, FlowItem, FlowPayload, FlowType, StateMachineShared, Trigger};
use crate::transport::TransportMethod;
use crate::triggers::{check_condition, payload_compatible};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RunOption {
  pub exec: String,
  pub args: Vec<String>,
  pub env: Option<HashMap<String, String>>,
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
  pub child: Option<Child>,
  pub state: ServiceState,
  pub retry_count: u32,
  pub stop_time: Option<Instant>,
  pub manually_stopped: bool,
}

impl ChildInstance {
  pub fn new(key: String, child: Option<Child>) -> Self {
    Self {
      key,
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

#[model(
  meta_name = name,
  meta_fields(
    name, run, after, branching, restart, start_on, stop_on, on_start, on_stop,
    transport
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
}

impl Default for ServiceRuntime {
  fn default() -> Self {
    Self { event_rx: None }
  }
}

impl ServiceRuntime {
  pub fn spawn_all(
    &self,
    service: &Service,
    branch_key: Option<&str>,
    sm_shared: Option<&StateMachineShared>,
  ) -> anyhow::Result<Vec<ChildInstance>> {
    service
      .metadata
      .run
      .as_many()
      .map(|run| self.spawn_process(service, run, branch_key, sm_shared))
      .collect()
  }

  pub fn spawn_service(
    &self,
    service: &mut Service,
    log: &LogHandle,
    sm_shared: Option<&StateMachineShared>,
  ) -> anyhow::Result<()> {
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service started",
      self.log_fields(service, "start"),
    );

    let instances = self.spawn_all(service, None, sm_shared)?;
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
    branch_key: Option<&str>,
    sm_shared: Option<&StateMachineShared>,
  ) -> anyhow::Result<ChildInstance> {
    let mut args = run.args.clone();
    let mut envs = run.env.clone().unwrap_or_default();

    if let Some(transport) = &service.metadata.transport {
      if let Some(sm_shared) = sm_shared {
        if let Ok(sm) = sm_shared.read() {
          let resolve_state = |name: &str| -> Option<String> {
            sm.states
              .get(name)
              .and_then(|v| v.first())
              .map(|x| x.payload.to_string_payload())
          };

          match transport {
            crate::transport::TransportMethod::Options { id, options } if id.0 == "env" => {
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
            crate::transport::TransportMethod::Options { id, options } if id.0 == "args" => {
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
    }

    let child = unsafe {
      let mut cmd = Command::new(&run.exec);
      cmd
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .pre_exec(|| {
          libc::setsid();
          Ok(())
        });
      if let Some(key) = branch_key {
        cmd.env("RIND_BRANCH_KEY", key);
      }
      if !envs.is_empty() {
        cmd.envs(&envs);
      }
      cmd.spawn()?
    };

    Ok(ChildInstance::new(
      branch_key.map(|x| x.to_string()).unwrap_or_default(),
      Some(child),
    ))
  }

  pub fn start_service(
    &self,
    service: &mut Service,
    log: &LogHandle,
    sm_shared: Option<&StateMachineShared>,
    dispatch: &RuntimeDispatcher,
  ) {
    if let Some(inst) = service.instances.as_one() {
      if inst.state == ServiceState::Active || inst.state == ServiceState::Starting {
        return;
      }
    }

    match self.spawn_service(service, log, sm_shared) {
      Ok(_) => {
        if let Some(inst) = service.instances.as_one_mut() {
          inst.state = ServiceState::Active;
          self.run_triggers(service.metadata.on_start.as_ref(), dispatch);
        }
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
  ) {
    for inst in service.instances.iter_mut() {
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
    log.log(
      LogLevel::Info,
      "service-runtime",
      "service stopping",
      fields,
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
    let inst = &mut service.instances.0[idx];

    if matches!(inst.state, ServiceState::Active | ServiceState::Stopping) {
      self.run_triggers(service.metadata.on_stop.as_ref(), dispatch);
    }

    inst.state = ServiceState::Exited(code);
    inst.child = None;

    if inst.manually_stopped {
      return Some(ServiceExitAction::StopDependents);
    }

    let restart_policy = service.metadata.restart.as_ref();
    match restart_policy {
      Some(RestartPolicy::Bool(true)) => Some(ServiceExitAction::Restart),
      Some(RestartPolicy::OnFailure { max_retries }) => {
        if code != 0 && *max_retries > 0 && inst.retry_count < *max_retries {
          inst.retry_count += 1;
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
}

enum ServiceExitAction {
  Restart,
  StopDependents,
}

#[derive(Default, Serialize, Deserialize)]
pub struct EmitTrigger {
  pub service: Option<String>,
  pub state: Option<String>,
  pub payload: Option<FlowPayload>,
  pub action: FlowAction,
}

impl Into<serde_json::Value> for EmitTrigger {
  fn into(self) -> serde_json::Value {
    serde_json::to_value(self).unwrap_or_default()
  }
}

impl Runtime for ServiceRuntime {
  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    match action {
      "watch_events" => {
        if let Some(event_bus) = ctx.scope.get::<EventBus>() {
          self.event_rx = Some(event_bus.subscribe::<rind_core::prelude::FlowEvent>());
        }
      }
      "drain_events" => {
        if let Some(rx) = &self.event_rx {
          // TODO: Fix this part
          // let mut triggered = false;
          while let Some(w) = rx.try_recv() {
            // triggered = true;
            let mut trig = EmitTrigger::default();
            trig.state = Some(w.name);
            trig.payload = Some(FlowPayload::from_json(Some(w.payload)));
            trig.action = w.action;
            let val: serde_json::Value = trig.into();
            let _ = dispatch.dispatch("services", "evaluate_triggers", val.into());
          }
        }
      }
      "evaluate_triggers" => {
        let emit_trig = payload.r#as::<EmitTrigger>().unwrap_or_default();

        let sm_shared = ctx.scope.get::<StateMachineShared>().cloned();
        let Some(sm_lock) = &sm_shared else {
          return Ok(());
        };
        let Ok(sm) = sm_lock.read() else {
          return Ok(());
        };

        let services = ctx
          .registry
          .metadata
          .items::<Service>("units")
          .unwrap_or(Vec::new());

        for (unit, service) in services {
          let mut is_running = false;

          if let Some(instances) = ctx
            .registry
            .instances
            .get_mut(&format!("units@{}@{}", unit, service.name))
          {
            for instance in instances.iter_mut() {
              if let Some(service) = instance.downcast_mut::<Service>() {
                let should_stop = service
                  .metadata
                  .stop_on
                  .as_ref()
                  .map(|conds| {
                    conds
                      .iter()
                      .any(|cond| crate::flow::condition_is_active(&sm, cond, None))
                  })
                  .unwrap_or(false)
                  || {
                    if let Some(ref state) = emit_trig.state
                      && emit_trig.action == FlowAction::Revert
                    {
                      if let Some(ref states) = service.metadata.start_on
                        && let Some(qry) = states.iter().find(|st| st.name() == state)
                      {
                        // TODO: This will work but, instead use the payload matcher to match the payload
                        // and quit
                        if let Some(ref p) = emit_trig.payload {
                          let instance = FlowInstance {
                            name: state.clone(),
                            payload: p.clone(),
                            r#type: FlowType::State,
                          };
                          check_condition(qry, &instance)
                        } else {
                          true
                        }
                        // !crate::flow::condition_is_active(&sm, qry, emit_trig.payload.as_ref())
                      } else {
                        false
                      }
                    } else {
                      false
                    }
                  };

                is_running = service
                  .instances
                  .iter()
                  .any(|i| i.state == ServiceState::Active || i.state == ServiceState::Starting);

                if should_stop && is_running {
                  self.stop_service(service, StopMode::Graceful, log, dispatch);
                }
              }
            }
          }

          let should_start = service
            .start_on
            .as_ref()
            .map(|conds| {
              conds
                .iter()
                .all(|cond| crate::flow::condition_is_active(&sm, cond, None))
            })
            .unwrap_or(false)
            || {
              if let Some(ref state) = emit_trig.state
                && emit_trig.action == FlowAction::Apply
              {
                if let Some(ref states) = service.start_on
                  && let Some(qry) = states.iter().find(|st| st.name() == state)
                {
                  crate::flow::condition_is_active(&sm, qry, emit_trig.payload.as_ref())
                } else {
                  false
                }
              } else {
                false
              }
            };

          if should_start && !is_running {
            let ser =
              ctx
                .registry
                .instantiate_one("units", &format!("{unit}@{}", service.name), |x| {
                  Ok(Service::new(x))
                })?;

            self.start_service(ser, log, sm_shared.as_ref(), dispatch);
          }
        }
      }
      "start" => {
        let name = payload.get::<String>("name")?;
        let service = ctx
          .registry
          .instantiate_one::<Service>("units", &name, |x| Ok(Service::new(x)))?;
        let sm_shared = ctx.scope.get::<StateMachineShared>().cloned();
        self.start_service(service, log, sm_shared.as_ref(), dispatch);
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
        self.stop_service(service, mode, log, dispatch);
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

        let sm_shared = ctx.scope.get::<StateMachineShared>().cloned();

        for (full_name, svc_meta) in &all_services {
          if let Some(afters) = &svc_meta.after {
            pending.push((full_name.clone(), afters.clone(), svc_meta.clone()));
          } else {
            let service = ctx
              .registry
              .instantiate_one::<Service>("units", &full_name, |x| Ok(Service::new(x)))?;
            self.start_service(service, log, sm_shared.as_ref(), dispatch);
            started.insert(full_name.clone());
          }
        }

        loop {
          let mut progress = false;
          pending.retain(|(name, afters, _meta)| {
            if afters.iter().all(|a| started.contains(a)) {
              if let Ok(service) = ctx
                .registry
                .instantiate_one::<Service>("units", name, |x| Ok(Service::new(x)))
              {
                self.start_service(service, log, sm_shared.as_ref(), dispatch);
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

        for key in keys {
          if let Some(instances) = ctx.registry.instances.get_mut(&key) {
            for instance in instances.iter_mut() {
              if let Some(service) = instance.downcast_mut::<Service>() {
                if let Some(exit_action) = self.handle_child_exit(service, pid, code, log, dispatch)
                {
                  match exit_action {
                    ServiceExitAction::Restart => {
                      let sm_shared = ctx.scope.get::<StateMachineShared>().cloned();
                      self.start_service(service, log, sm_shared.as_ref(), dispatch);
                    }
                    ServiceExitAction::StopDependents => {}
                  }
                }
              }
            }
          }
        }
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
    Ok(())
  }

  fn id(&self) -> &str {
    "services"
  }
}
