use crate::context::ScopeBuilder;
use crate::error::CoreError;
use crate::logging::{LogConfig, LogHandle, start_logger};
use crate::notifier::Notifier;
use crate::orchestrator::{BootCycle, BootPhase, OrchestratorContext, OrchestratorStore};
use crate::prelude::{RELOAD_STATIC, Resources};
use crate::registry::{InstanceMap, MetadataRegistry};
use crate::runtime::{RuntimeHandle, RuntimePayload, start_runtime};
use crate::types::Void;

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

  pub fn start_logger(&self) -> LogHandle {
    start_logger(LogConfig::default())
  }

  pub fn init_runtime(&self, log: LogHandle, notifier: Option<Notifier>) -> RuntimeHandle {
    start_runtime(log, self.orchestrators.runtimes(), notifier)
  }

  pub fn pre_boot(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    resources: &mut Resources,
    log: LogHandle,
  ) -> Result<Void, CoreError> {
    let runtime = RuntimeHandle::mock(log);
    for phase in [BootPhase::Start, BootPhase::End] {
      let mut ctx = OrchestratorContext {
        context_id: 0,
        metadata,
        instances,
        runtime: &runtime,
        resources,
      };

      self
        .orchestrators
        .run_cycle_phase(BootCycle::PreBoot, phase, &mut ctx)?;
    }

    Ok(Void)
  }

  pub fn run(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
    resources: &mut Resources,
  ) -> Result<Void, CoreError> {
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
            resources,
          };
          self.orchestrators.run_cycle_phase(cycle, phase, &mut ctx)?;
        }

        runtime.flush_context(context_id, metadata, resources)?;

        if cycle == BootCycle::Runtime && phase == BootPhase::Start {
          self.persistent_context_ids.push(context_id);
        }
      }
    }

    Ok(Void)
  }

  pub fn pump_once(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
    resources: &mut Resources,
  ) -> Result<Void, CoreError> {
    let context_ids = self.persistent_context_ids.clone();
    for context_id in context_ids {
      for phase in [BootPhase::Start, BootPhase::End] {
        let mut ctx = OrchestratorContext {
          context_id,
          metadata,
          instances,
          runtime,
          resources,
        };

        self
          .orchestrators
          .run_cycle_phase(BootCycle::Pump, phase, &mut ctx)?;
        runtime.flush_context(context_id, metadata, resources)?;
      }
    }

    Ok(Void)
  }

  pub fn primary_context_id(&self) -> Option<usize> {
    self.persistent_context_ids.first().copied()
  }

  pub fn reload_units_collection(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
    resources: &mut Resources,
  ) -> Result<Void, CoreError> {
    let mut reload_static = RELOAD_STATIC.lock().unwrap();
    let to_remove = if *reload_static {
      *reload_static = false;
      metadata.metadata_names().collect::<Vec<_>>()
    } else {
      metadata
        .metadata_names()
        .filter(|name| name.as_str() != "static")
        .collect::<Vec<_>>()
    };
    let types = metadata.indexes.keys().cloned().collect::<Vec<_>>();
    for name in to_remove {
      let context_id = self.primary_context_id().unwrap_or(0);
      for t in types.iter() {
        if let Some((r, a)) = metadata.stoppers.get(t) {
          let _ = runtime.dispatch(
            r,
            a,
            RuntimePayload::default().insert("scope", name.clone()),
            context_id,
          );
        }
      }
      metadata.remove_metadata(name);
    }

    for phase in [BootPhase::Start, BootPhase::End] {
      let context_id = self.alloc_context_id();
      let mut builder = ScopeBuilder::default();
      self
        .orchestrators
        .build_scope_cycle_phase(BootCycle::Collect, phase, &mut builder)?;
      runtime.register_scopes(context_id, builder.build())?;

      {
        let mut ctx = OrchestratorContext {
          context_id,
          metadata,
          instances,
          runtime,
          resources,
        };
        self
          .orchestrators
          .run_cycle_phase(BootCycle::Collect, phase, &mut ctx)?;
      }

      runtime.flush_context(context_id, metadata, resources)?;
    }

    Ok(Void)
  }
}
