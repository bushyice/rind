use rind_core::prelude::{LogConfig, LogHandle, RuntimeCommand, start_logger, start_runtime};
use rind_core::context::{RuntimeContext, ScopeBuilder};
use rind_core::error::CoreError;
use rind_core::metadata::Metadata;
use rind_core::orchestrator::{
  BootCycle, BootPhase, Orchestrator, OrchestratorContext, OrchestratorWhen,
};
use rind_core::registry::{InstanceMap, MetadataRegistry};
use rind_core::runtime::{Runtime, RuntimeDispatcher, RuntimePayload};
use rind_core::types::Void;
use rind_core::boot::BootEngine;
use rind_core::prelude::Resources;

use std::sync::mpsc::{self, Sender};
use std::time::Duration;

struct ScopeOrchestrator {
  phase: BootPhase,
  runtime_id: String,
  value: String,
}

impl ScopeOrchestrator {
  fn new(phase: BootPhase, runtime_id: &str, value: &str) -> Self {
    Self {
      phase,
      runtime_id: runtime_id.to_string(),
      value: value.to_string(),
    }
  }
}

impl Orchestrator for ScopeOrchestrator {
  fn id(&self) -> &str {
    self.value.as_str()
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect, BootCycle::Runtime],
      phase: self.phase,
    }
  }

  fn build_scope(&mut self, builder: &mut ScopeBuilder) -> Result<Void, CoreError> {
    let runtime_id = self.runtime_id.clone();
    let value = self.value.clone();
    builder.insert(runtime_id, || value);
    Ok(Void)
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    ctx.dispatch(self.runtime_id.as_str(), "boot", Default::default())
  }
}

struct ScopeReaderRuntime {
  id: String,
  tx: Sender<String>,
}

impl ScopeReaderRuntime {
  fn new(id: &str, tx: Sender<String>) -> Self {
    Self {
      id: id.to_string(),
      tx,
    }
  }
}

impl Runtime for ScopeReaderRuntime {
  fn id(&self) -> &str {
    self.id.as_str()
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    if action == "boot" {
      let value =
        ctx.scope.get::<String>().cloned().ok_or_else(|| {
          CoreError::InvalidState("missing String in runtime scope".to_string())
        })?;
      let _ = self.tx.send(value);
    }
    Ok(None)
  }
}

struct PingRuntime;

impl Runtime for PingRuntime {
  fn id(&self) -> &str {
    "ping"
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    _ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    if action == "kick" {
      dispatch.dispatch(
        "pong",
        "from_ping",
        RuntimePayload::default().insert("something", 1),
      )?;
    }
    Ok(None)
  }
}

struct PongRuntime {
  tx: Sender<u32>,
}

impl Runtime for PongRuntime {
  fn id(&self) -> &str {
    "pong"
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    if action == "from_ping" {
      let value = ctx
        .scope
        .get::<u32>()
        .copied()
        .ok_or_else(|| CoreError::InvalidState("missing u32 in runtime scope".to_string()))?;
      let _ = self.tx.send(value);
    }
    Ok(None)
  }
}

struct KickoffOrchestrator;

impl Orchestrator for KickoffOrchestrator {
  fn id(&self) -> &str {
    "kickoff"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn build_scope(&mut self, builder: &mut ScopeBuilder) -> Result<Void, CoreError> {
    builder.insert("pong", || 7u32);
    Ok(Void)
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    ctx.dispatch("ping", "kick", Default::default())
  }
}

struct ResourceRuntime;
impl Runtime for ResourceRuntime {
  fn id(&self) -> &str {
    "resource"
  }
  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    if action == "register" {
      ctx.resources.register_resource(42);
    }
    Ok(None)
  }
}

struct ResourceOrchestrator;
impl Orchestrator for ResourceOrchestrator {
  fn id(&self) -> &str {
    "resource"
  }
  fn depends_on(&self) -> &[&str] {
    &[]
  }
  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }
  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    ctx.dispatch("resource", "register", Default::default())
  }
}

fn logger_for_tests() -> LogHandle {
  let log_dir = std::env::temp_dir().join(format!("rind-core-boot-tests-{}", std::process::id()));
  start_logger(LogConfig {
    dir: log_dir,
    ..LogConfig::default()
  })
}

#[test]
fn runtime_can_dispatch_to_another_runtime() {
  let log = logger_for_tests();
  let (tx, rx) = mpsc::channel::<u32>();
  let runtime = start_runtime(
    log,
    vec![Box::new(PingRuntime), Box::new(PongRuntime { tx })],
    None,
  );

  let mut boot = BootEngine::default();
  boot.orchestrators.push(KickoffOrchestrator);

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let mut resources = Resources::default();
  boot
    .run(&mut metadata, &mut instances, &runtime, &mut resources)
    .expect("boot run should succeed");

  let value = rx
    .recv_timeout(Duration::from_secs(2))
    .expect("pong should receive runtime-to-runtime dispatch");
  assert_eq!(value, 7u32);

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn boot_applies_phase_specific_scope_contexts() {
  let log = logger_for_tests();
  let (tx, rx) = mpsc::channel::<String>();
  let runtime = start_runtime(
    log,
    vec![Box::new(ScopeReaderRuntime::new("alpha", tx))],
    None,
  );

  let mut boot = BootEngine::default();
  boot.orchestrators.push(ScopeOrchestrator::new(
    BootPhase::Start,
    "alpha",
    "from_start",
  ));
  boot
    .orchestrators
    .push(ScopeOrchestrator::new(BootPhase::End, "alpha", "from_end"));

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let mut resources = Resources::default();
  boot
    .run(&mut metadata, &mut instances, &runtime, &mut resources)
    .expect("boot run should succeed");

  let first = rx
    .recv_timeout(Duration::from_secs(2))
    .expect("start message");
  let second = rx
    .recv_timeout(Duration::from_secs(2))
    .expect("end message");
  assert_eq!(first, "from_start".to_string());
  assert_eq!(second, "from_end".to_string());

  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn runtime_can_register_resources() {
  let log = logger_for_tests();
  let runtime = start_runtime(log, vec![Box::new(ResourceRuntime)], None);
  let mut boot = BootEngine::default();
  boot.orchestrators.push(ResourceOrchestrator);
  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let mut resources = Resources::default();
  boot
    .run(&mut metadata, &mut instances, &runtime, &mut resources)
    .expect("boot run should succeed");
  assert_eq!(resources.unwatched_fds(), vec![42]);
  let _ = runtime.send(RuntimeCommand::Stop);
}

#[test]
fn reload_units_collection_rebuilds_collect_cycle_metadata() {
  let log = logger_for_tests();
  let runtime = start_runtime(log, vec![], None);
  let mut boot = BootEngine::default();
  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let mut resources = Resources::default();

  let units = Metadata::new("units");
  metadata.insert_metadata(units);
  assert!(metadata.metadata("units").is_some());

  boot
    .reload_units_collection(&mut metadata, &mut instances, &runtime, &mut resources)
    .expect("reload units collect cycle should succeed");
  assert!(metadata.metadata("units").is_none());

  let _ = runtime.send(RuntimeCommand::Stop);
}
