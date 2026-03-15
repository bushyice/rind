use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use rind_core::prelude::*;

use crate::flow::{Signal, State, StateMachine, StateMachineShared};
use crate::mount::Mount;
use crate::services::Service;

pub const UNITS_META: &str = "units";
const BUILTIN_UNIT: &str = "__rind";

pub struct UnitsOrchestrator {
  units_dir: PathBuf,
  state_machine: StateMachineShared,
  state_persistence: Arc<RwLock<StatePersistence>>,
  event_bus: EventBus,
}

impl UnitsOrchestrator {
  pub fn new(units_dir: impl Into<PathBuf>) -> Self {
    Self {
      units_dir: units_dir.into(),
      state_machine: Arc::new(RwLock::new(StateMachine::default())),
      state_persistence: Arc::new(RwLock::new(StatePersistence::new(state_path()))),
      event_bus: EventBus::new(),
    }
  }

  fn load_all_units(&self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    let mut metadata = Metadata::new(UNITS_META)
      .of::<Service>("service")
      .of::<Mount>("mount")
      .of::<State>("state")
      .of::<Signal>("signal");

    let dir = std::fs::read_dir(&self.units_dir).map_err(|e| {
      CoreError::Custom(format!(
        "failed to read units dir {}: {e}",
        self.units_dir.display()
      ))
    })?;

    for entry in dir {
      let entry = entry.map_err(|e| CoreError::Custom(format!("dir entry error: {e}")))?;
      let path = entry.path();

      if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
        continue;
      }

      if path.extension().map_or(true, |ext| ext != "toml") {
        continue;
      }

      let group = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

      let content = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Custom(format!("failed to read unit file {}: {e}", path.display()))
      })?;

      ctx
        .metadata
        .load_group_from_toml(&mut metadata, &group, &content)
        .map_err(|e| {
          CoreError::Custom(format!("failed to parse unit file {}: {e}", path.display()))
        })?;
    }

    Self::add_builtin_defs(&mut metadata);

    ctx.metadata.insert_metadata(metadata);
    ctx.metadata.ensure_index_for_type::<Service>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Mount>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<State>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Signal>(UNITS_META)?;

    Ok(())
  }

  fn add_builtin_defs(metadata: &mut Metadata) {
    let builtin_toml = format!(
      r#"
[[state]]
name = "active"
payload = "string"

[[signal]]
name = "activate"
payload = "string"

[[signal]]
name = "deactivate"
payload = "string"
"#
    );

    let _ = metadata.from_toml(&builtin_toml, BUILTIN_UNIT);
  }
}

impl Orchestrator for UnitsOrchestrator {
  fn id(&self) -> &str {
    "units"
  }

  fn depends_on(&self) -> &[String] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect, BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    Vec::new()
  }

  fn preload(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    if ctx.metadata.metadata(UNITS_META).is_none() {
      self.load_all_units(ctx)?;
    }
    let loaded = self
      .state_persistence
      .write()
      .map_err(CoreError::custom)?
      .load()?;

    self
      .state_machine
      .write()
      .map_err(CoreError::custom)?
      .load_from_persistence(loaded);
    Ok(())
  }

  fn build_scope(&mut self, builder: &mut ScopeBuilder) -> Result<(), CoreError> {
    let sm = self.state_machine.clone();
    let eb = self.event_bus.clone();
    builder.insert_scope("flow", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<StateMachineShared>(sm.clone());
      scope.insert::<EventBus>(eb.clone());
      scope
    });

    let sm = self.state_machine.clone();
    let eb = self.event_bus.clone();
    builder.insert_scope("services", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<StateMachineShared>(sm.clone());
      scope.insert::<EventBus>(eb.clone());
      scope
    });

    let sm = self.state_machine.clone();
    let eb = self.event_bus.clone();
    builder.insert_scope("mounts", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<StateMachineShared>(sm.clone());
      scope.insert::<EventBus>(eb.clone());
      scope
    });

    let eb = self.event_bus.clone();
    builder.insert_scope("transport", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<EventBus>(eb.clone());
      scope
    });

    let eb = self.event_bus.clone();
    builder.insert_scope("reaper", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<EventBus>(eb.clone());
      scope
    });

    Ok(())
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    Ok(())
  }
}

fn state_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_STATE_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/var/lib/system-state")
  }
}
