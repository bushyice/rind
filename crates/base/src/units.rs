use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::flow::{Signal, State, StateMachine, StateMachineShared};
use crate::mount::Mount;
use crate::networking::NetworkConfig;
use crate::permissions::{PERM_LOGIN, PERM_RUN0, PERM_SYSTEM_SERVICES, Permission};
use crate::services::Service;
use crate::user::Run0QueueState;
use crate::variables::{Variable, VariableHeap, VariableHeapShared, variables_path};
use rind_core::prelude::*;
use rind_core::user::{PamHandle, UserStore};
use rind_ipc::recv::IpcSourcemap;

pub const UNITS_META: &str = "units";
const BUILTIN_UNIT: &str = "rind";

pub enum UnitExtensionAction {
  Metadata(Metadata),
  CreateIndex,
  BuiltIn(Metadata),
  LoadedUnits(Metadata),
}

impl UnitExtensionAction {
  pub fn as_metadata(self) -> Metadata {
    match self {
      UnitExtensionAction::Metadata(m) => m,
      UnitExtensionAction::LoadedUnits(m) => m,
      UnitExtensionAction::BuiltIn(m) => m,
      _ => Metadata::new("unknown"),
    }
  }
}

impl From<Metadata> for UnitExtensionAction {
  fn from(value: Metadata) -> Self {
    UnitExtensionAction::Metadata(value)
  }
}

impl From<()> for UnitExtensionAction {
  fn from(_value: ()) -> Self {
    UnitExtensionAction::CreateIndex
  }
}

pub type UnitExtension = fn(UnitExtensionAction) -> UnitExtensionAction;

pub struct UnitsOrchestrator {
  units_dir: PathBuf,
  state_machine: StateMachineShared,
  state_persistence: Arc<RwLock<StatePersistence>>,
  lifecycle: LifecycleQueue,
  event_bus: EventBus,
  users: UserStoreShared,
  permissions: PermissionStore,
  variable_heap: VariableHeapShared,
  extensions: Vec<UnitExtension>,
}

impl UnitsOrchestrator {
  pub fn new(units_dir: impl Into<PathBuf>) -> Self {
    let users = Arc::new(UserStore::load_system().unwrap_or_default());
    let mut heap = VariableHeap::new(variables_path());
    let _ = heap.load();
    Self {
      units_dir: units_dir.into(),
      state_machine: Arc::new(RwLock::new(StateMachine::default())),
      state_persistence: Arc::new(RwLock::new(StatePersistence::new(state_path()))),
      lifecycle: LifecycleQueue::default(),
      event_bus: EventBus::new(),
      permissions: PermissionStore::new(users.clone()),
      users,
      variable_heap: Arc::new(RwLock::new(heap)),
      extensions: Default::default(),
    }
  }

  pub fn insert_extension(&mut self, ext: UnitExtension) {
    self.extensions.push(ext);
  }

  pub fn lifecycle_queue(&self) -> LifecycleQueue {
    self.lifecycle.clone()
  }

  fn load_permissions(&self) -> Result<(), CoreError> {
    self
      .permissions
      .reg_perm(PERM_LOGIN, "Login")?
      .reg_perm(PERM_SYSTEM_SERVICES, "SystemServices")?
      .reg_perm(PERM_RUN0, "Run0")?;

    Ok(())
  }

  fn load_all_units(&self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    let mut metadata = Metadata::new(UNITS_META)
      .of::<Service>("service")
      .of::<Mount>("mount")
      .of::<NetworkConfig>("network")
      .of::<State>("state")
      .of::<Signal>("signal")
      .of::<Permission>("permission")
      .of::<Variable>("variable");

    for ext in self.extensions.iter() {
      metadata = ext(metadata.into()).as_metadata();
    }

    let dir = std::fs::read_dir(&self.units_dir).map_err(|e| {
      CoreError::Custom(format!(
        "failed to read units dir {}: {e}",
        self.units_dir.display()
      ))
    })?;

    for entry in dir {
      let entry = entry.map_err(|e| CoreError::Custom(format!("dir entry error: {e}")))?;
      let path = entry.path();

      if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
        if let Ok(sub_dir) = std::fs::read_dir(&path) {
          for sub_entry in sub_dir {
            let sub_entry =
              sub_entry.map_err(|e| CoreError::Custom(format!("dir entry error: {e}")))?;
            let sub_path = sub_entry.path();
            if !sub_entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
              continue;
            }
            if sub_path.extension().map_or(true, |ext| ext != "toml") {
              continue;
            }
            let group = format!(
              "{}/{}",
              path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
              sub_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
            );
            let content = std::fs::read_to_string(&sub_path).map_err(|e| {
              CoreError::Custom(format!(
                "failed to read unit file {}: {e}",
                sub_path.display()
              ))
            })?;
            ctx
              .metadata
              .load_group_from_toml(&mut metadata, &group, &content)
              .map_err(|e| {
                CoreError::Custom(format!(
                  "failed to parse unit file {}: {e}",
                  sub_path.display()
                ))
              })?;
          }
        }
        continue;
      }

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

      if content.contains("permission") {
        if let Some(group) = metadata.get_in_group::<Permission>(&group) {
          for perm in group {
            self
              .permissions
              .reg_perm(PermissionId(perm.id), perm.name.clone())?;
          }
        }
      }
    }

    Self::add_builtin_defs(&mut metadata);

    for ext in self.extensions.iter() {
      metadata = ext(UnitExtensionAction::BuiltIn(metadata)).as_metadata();
    }

    if let Ok(mut heap) = self.variable_heap.write() {
      for group in metadata.groups() {
        if let Some(vars) = metadata.get_in_group::<Variable>(group) {
          for var in vars {
            heap.register(&var.name, var.default.clone());
          }
        }
      }
    }

    for ext in self.extensions.iter() {
      metadata = ext(UnitExtensionAction::LoadedUnits(metadata)).as_metadata();
    }

    ctx.metadata.insert_metadata(metadata);
    ctx.metadata.ensure_index_for_type::<Service>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Mount>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<State>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Signal>(UNITS_META)?;

    for ext in self.extensions.iter() {
      ext(UnitExtensionAction::CreateIndex);
    }

    Ok(())
  }

  fn add_builtin_defs(metadata: &mut Metadata) {
    let builtin_toml = r#"
[[state]]
name = "active"
payload = "string"

[[state]]
name = "user_session"
payload = "json"
branch = ["session_id"]

[[state]]
name = "user_auto_login"
payload = "json"
branch = ["tty"]

[[signal]]
name = "activate"
payload = "string"

[[signal]]
name = "deactivate"
payload = "string"

[[signal]]
name = "request_login"
payload = "json"

[[signal]]
name = "request_logout"
payload = "json"

[[state]]
name = "net-interface"
payload = "json"
branch = ["name"]

[[state]]
name = "online"
payload = "none"

[[state]]
name = "net-configured"
payload = "json"
branch = ["name"]

[[state]]
name = "net-dns_ready"
payload = "none"

[[state]]
name = "firewall"
payload = "none"
"#;

    let _ = metadata.from_toml(builtin_toml, BUILTIN_UNIT);
  }
}

impl Orchestrator for UnitsOrchestrator {
  fn id(&self) -> &str {
    "units"
  }

  fn depends_on(&self) -> &[&str] {
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
      let mut vars = self.variable_heap.write().unwrap();
      vars.load()?;
      drop(vars);
      self.load_all_units(ctx)?;
      self.load_permissions()?;
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
    let pam_handle = Arc::new(PamHandle::new(self.users.clone()));

    let persistence = self.state_persistence.clone();
    builder.insert_scope("flow", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<Arc<RwLock<StatePersistence>>>(persistence.clone());
      scope
    });

    let pam = pam_handle.clone();
    builder.insert_scope("services", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<Arc<PamHandle>>(pam.clone());
      scope
    });

    let pam = pam_handle.clone();
    builder.insert_scope("ipc", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<Arc<PamHandle>>(pam.clone());
      scope
    });

    let pam = pam_handle.clone();
    builder.insert_scope("user", move || {
      let mut scope = RuntimeScope::default();
      scope.insert::<Arc<PamHandle>>(pam.clone());
      scope
    });

    // Why do all the monsters come out at night?
    // Why do we sleep where we want to hide?
    // Why do I run back to you like I don't mind if you fuck up my life?
    // Why am I a sucker for all your lies?
    // Strung out like laundry on every line.
    // Why do I run back to you like I don't mind if you fuck up my life?
    let eb = self.event_bus.clone();
    let permissions = self.permissions.clone();
    let sm = self.state_machine.clone();
    let ipcmap = IpcSourcemap::default();
    let run0_queue: Run0QueueState = Arc::new(std::sync::Mutex::new(Default::default()));
    let variable_heap = self.variable_heap.clone();
    let lifecycle = self.lifecycle.clone();
    builder.globals(move |scope| {
      scope.insert::<IpcSourcemap>(ipcmap.clone());
      scope.insert::<EventBus>(eb.clone());
      scope.insert::<StateMachineShared>(sm.clone());
      scope.insert::<PermissionStore>(permissions.clone());
      scope.insert::<Run0QueueState>(run0_queue.clone());
      scope.insert::<VariableHeapShared>(variable_heap.clone());
      scope.insert::<LifecycleQueue>(lifecycle.clone());
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
