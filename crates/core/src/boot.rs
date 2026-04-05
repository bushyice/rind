use crate::context::ScopeBuilder;
use crate::error::CoreError;
use crate::logging::{LogConfig, start_logger};
use crate::orchestrator::{BootCycle, BootPhase, OrchestratorContext, OrchestratorStore};
use crate::registry::{InstanceMap, MetadataRegistry};
use crate::runtime::{RuntimeHandle, start_runtime};

pub struct BootEngine {
  pub orchestrators: OrchestratorStore,
  next_context_id: usize,
  persistent_context_ids: Vec<usize>,
}

impl Default for BootEngine {
  fn default() -> Self {
    Self {
      orchestrators: OrchestratorStore::default(),
      next_context_id: 1,
      persistent_context_ids: Vec::new(),
    }
  }
}

impl BootEngine {
  fn alloc_context_id(&mut self) -> usize {
    let current = self.next_context_id;
    self.next_context_id = self.next_context_id.saturating_add(1);
    current
  }

  pub fn init_runtime(&self) -> RuntimeHandle {
    let log = start_logger(LogConfig::default());

    start_runtime(log, self.orchestrators.runtimes())
  }

  pub fn run(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
  ) -> Result<(), CoreError> {
    self.persistent_context_ids.clear();

    for cycle in [
      BootCycle::Collect,
      BootCycle::Runtime,
      BootCycle::PostRuntime,
    ] {
      for phase in [BootPhase::Start, BootPhase::End] {
        let context_id = self.alloc_context_id();
        let mut builder = ScopeBuilder::default();
        self
          .orchestrators
          .build_scope_cycle_phase(cycle, phase, &mut builder)?;
        runtime.register_scopes(context_id, builder.build())?;

        {
          let mut ctx = OrchestratorContext {
            context_id,
            metadata,
            instances,
            runtime,
          };
          self.orchestrators.run_cycle_phase(cycle, phase, &mut ctx)?;
        }

        runtime.flush_context(context_id, metadata)?;

        if cycle == BootCycle::Runtime && phase == BootPhase::Start {
          self.persistent_context_ids.push(context_id);
        }
      }
    }

    Ok(())
  }

  pub fn pump_once(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
  ) -> Result<(), CoreError> {
    let context_ids = self.persistent_context_ids.clone();
    for context_id in context_ids {
      for phase in [BootPhase::Start, BootPhase::End] {
        let mut ctx = OrchestratorContext {
          context_id,
          metadata,
          instances,
          runtime,
        };

        self
          .orchestrators
          .run_cycle_phase(BootCycle::Pump, phase, &mut ctx)?;
        runtime.flush_context(context_id, metadata)?;
      }
    }

    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use std::sync::mpsc::{self, Sender};
  use std::time::Duration;

  use serde_json::json;

  use crate::context::{RuntimeContext, ScopeBuilder};
  use crate::logging::{LogConfig, LogHandle, start_logger};
  use crate::orchestrator::{
    BootCycle, BootPhase, Orchestrator, OrchestratorContext, OrchestratorWhen,
  };
  use crate::registry::{InstanceMap, MetadataRegistry};
  use crate::runtime::{Runtime, RuntimeCommand, RuntimeDispatcher, RuntimePayload, start_runtime};

  use super::*;

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

    fn depends_on(&self) -> &[String] {
      &[]
    }

    fn when(&self) -> OrchestratorWhen<'static> {
      OrchestratorWhen {
        cycle: &[BootCycle::Collect, BootCycle::Runtime],
        phase: self.phase,
      }
    }

    fn build_scope(&mut self, builder: &mut ScopeBuilder) -> Result<(), CoreError> {
      let runtime_id = self.runtime_id.clone();
      let value = self.value.clone();
      builder.insert(runtime_id, || value);
      Ok(())
    }

    fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
      ctx.dispatch(self.runtime_id.as_str(), "boot", json!({}))
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
    ) -> Result<(), CoreError> {
      if action == "boot" {
        let value =
          ctx.scope.get::<String>().cloned().ok_or_else(|| {
            CoreError::InvalidState("missing String in runtime scope".to_string())
          })?;
        let _ = self.tx.send(value);
      }
      Ok(())
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
    ) -> Result<(), CoreError> {
      if action == "kick" {
        dispatch.dispatch("pong", "from_ping", json!({ "hop": 1 }).into())?;
      }
      Ok(())
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
    ) -> Result<(), CoreError> {
      if action == "from_ping" {
        let value = ctx
          .scope
          .get::<u32>()
          .copied()
          .ok_or_else(|| CoreError::InvalidState("missing u32 in runtime scope".to_string()))?;
        let _ = self.tx.send(value);
      }
      Ok(())
    }
  }

  struct KickoffOrchestrator;

  impl Orchestrator for KickoffOrchestrator {
    fn id(&self) -> &str {
      "kickoff"
    }

    fn depends_on(&self) -> &[String] {
      &[]
    }

    fn when(&self) -> OrchestratorWhen<'static> {
      OrchestratorWhen {
        cycle: &[BootCycle::Runtime],
        phase: BootPhase::Start,
      }
    }

    fn build_scope(&mut self, builder: &mut ScopeBuilder) -> Result<(), CoreError> {
      builder.insert("pong", || 7u32);
      Ok(())
    }

    fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
      ctx.dispatch("ping", "kick", json!({}))
    }
  }

  fn logger_for_tests() -> LogHandle {
    let log_dir = std::env::temp_dir().join(format!("rind-core-tests-{}", std::process::id()));
    start_logger(LogConfig {
      dir: log_dir,
      ..LogConfig::default()
    })
  }

  #[test]
  fn boot_builds_scope_and_runtime_reads_it() {
    let log = logger_for_tests();
    let (tx, rx) = mpsc::channel::<String>();
    let runtime = start_runtime(log, vec![Box::new(ScopeReaderRuntime::new("alpha", tx))]);

    let mut boot = BootEngine::default();
    boot
      .orchestrators
      .push(ScopeOrchestrator::new(BootPhase::Start, "alpha", "hello"));

    let mut metadata = MetadataRegistry::default();
    let mut instances = InstanceMap::default();
    boot
      .run(&mut metadata, &mut instances, &runtime)
      .expect("boot run should succeed");

    let value = rx
      .recv_timeout(Duration::from_secs(2))
      .expect("runtime should receive scoped value");
    assert_eq!(value, "hello".to_string());

    let _ = runtime.send(RuntimeCommand::Stop);
  }

  #[test]
  fn runtime_can_dispatch_to_another_runtime() {
    let log = logger_for_tests();
    let (tx, rx) = mpsc::channel::<u32>();
    let runtime = start_runtime(
      log,
      vec![Box::new(PingRuntime), Box::new(PongRuntime { tx })],
    );

    let mut boot = BootEngine::default();
    boot.orchestrators.push(KickoffOrchestrator);

    let mut metadata = MetadataRegistry::default();
    let mut instances = InstanceMap::default();
    boot
      .run(&mut metadata, &mut instances, &runtime)
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
    let runtime = start_runtime(log, vec![Box::new(ScopeReaderRuntime::new("alpha", tx))]);

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
    boot
      .run(&mut metadata, &mut instances, &runtime)
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
}
