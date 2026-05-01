use std::path::PathBuf;
use std::sync::Arc;

use crate::flow::{Signal, State, StateMachine, state_path};
use crate::mount::Mount;
use crate::networking::NetworkConfig;
use crate::permissions::{PERM_LOGIN, PERM_RUN0, PERM_SYSTEM_SERVICES, Permission};
use crate::services::Service;
use crate::sockets::Socket;
use crate::timers::Timer;
use crate::user::Run0QueueState;
use crate::variables::{Variable, VariableHeap, variables_path};
use rind_core::prelude::*;
use rind_core::user::{PamHandle, UserStore};
use rind_ipc::recv::IpcSourcemap;

pub const UNITS_META: &str = "units";
const BUILTIN_UNIT: &str = "rind";

pub struct UnitsOrchestrator {
  units_dir: PathBuf,
  users: UserStoreShared,
}

impl UnitsOrchestrator {
  pub fn new(units_dir: impl Into<PathBuf>) -> Self {
    let users = Arc::new(UserStore::load_system().unwrap_or_default());
    Self {
      units_dir: units_dir.into(),
      users,
    }
  }

  fn load_permissions(&self, permissions: &PermissionStore) -> Result<(), CoreError> {
    permissions
      .reg_perm(PERM_LOGIN, "Login")?
      .reg_perm(PERM_SYSTEM_SERVICES, "SystemServices")?
      .reg_perm(PERM_RUN0, "Run0")?;

    Ok(())
  }

  fn load_all_units(
    &self,
    ctx: &mut OrchestratorContext<'_>,
    permissions: &PermissionStore,
  ) -> Result<(), CoreError> {
    let mut metadata = Metadata::new(UNITS_META)
      .of::<Service>("service")
      .of::<Timer>("timer")
      .of::<Mount>("mount")
      .of::<Socket>("socket")
      .of::<NetworkConfig>("network")
      .of::<State>("state")
      .of::<Signal>("signal")
      .of::<Permission>("permission")
      .of::<Variable>("variable");

    metadata = EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .resolve("component", metadata)
    })?;

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
            let group = Ustr::from(format!(
              "{}/{}",
              path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown"),
              sub_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
            ));
            let content = std::fs::read_to_string(&sub_path).map_err(|e| {
              CoreError::Custom(format!(
                "failed to read unit file {}: {e}",
                sub_path.display()
              ))
            })?;
            ctx
              .metadata
              .load_group_from_toml(&mut metadata, group, &content)
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

      let group = Ustr::from(
        path
          .file_stem()
          .and_then(|s| s.to_str())
          .unwrap_or("unknown"),
      );

      let content = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Custom(format!("failed to read unit file {}: {e}", path.display()))
      })?;

      ctx
        .metadata
        .load_group_from_toml(&mut metadata, group.clone(), &content)
        .map_err(|e| {
          CoreError::Custom(format!("failed to parse unit file {}: {e}", path.display()))
        })?;

      if content.contains("permission") {
        if let Some(items) = metadata.get_in_group::<Permission>(group) {
          for perm in items {
            permissions.reg_perm(PermissionId(perm.id), perm.name.clone())?;
          }
        }
      }
    }

    Self::add_builtin_defs(&mut metadata);

    metadata = EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .resolve("built_in", metadata)
    })?;

    EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .act("loaded_units", &mut metadata)
    })?;

    ctx.metadata.insert_metadata(metadata);
    ctx.metadata.ensure_index_for_type::<Service>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Mount>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Socket>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Timer>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<State>(UNITS_META)?;
    ctx.metadata.ensure_index_for_type::<Signal>(UNITS_META)?;

    EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .act("create_index", ctx.metadata)
    })?;

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
    let metadata = &*ctx.metadata;
    let users = self.users.clone();
    let permissions = ctx.runtime.with_instances(|instances| {
      let mut registry = InstanceRegistry::new(metadata, instances);
      registry
        .singleton_or_insert_with::<PermissionStore>(PermissionStore::KEY, || {
          PermissionStore::new(users.clone())
        })
        .clone()
    })?;

    if ctx.metadata.metadata(UNITS_META).is_none() {
      self.load_all_units(ctx, &permissions)?;
      self.load_permissions(&permissions)?;
    }

    let metadata = &*ctx.metadata;
    let users = self.users.clone();
    ctx
      .runtime
      .with_instances(|instances| -> std::result::Result<(), CoreError> {
        let mut registry = InstanceRegistry::new(metadata, instances);

        let _ = registry.singleton_or_insert_with::<StateMachine>(StateMachine::KEY, || {
          let mut state = StateMachine::from_persistence(StatePersistence::new(state_path()));
          let _ = state.load_from_persistence();
          state
        });

        let _ = registry.singleton_or_insert_with::<Arc<PamHandle>>(PamHandle::KEY, || {
          Arc::new(PamHandle::new(users.clone()))
        });

        let variable_heap =
          registry.singleton_or_insert_with::<VariableHeap>(VariableHeap::KEY, || {
            let mut heap = VariableHeap::new(variables_path());
            let _ = heap.load();
            heap
          });

        if let Some(units) = ctx.metadata.metadata(UNITS_META) {
          for group in units.groups() {
            if let Some(vars) = units.get_in_group::<Variable>(group) {
              for var in vars {
                variable_heap.register(var.name.clone(), var.default.clone(), var.env.clone());
              }
            }
          }
        }

        Ok(())
      })??;

    Ok(())
  }

  fn build_scope(&mut self, builder: &mut ScopeBuilder) -> Result<(), CoreError> {
    // Why do all the monsters come out at night?
    // Why do we sleep where we want to hide?
    // Why do I run back to you like I don't mind if you fuck up my life?
    // Why am I a sucker for all your lies?
    // Strung out like laundry on every line.
    // Why do I run back to you like I don't mind if you fuck up my life?
    let ipcmap = IpcSourcemap::default();
    let run0_queue: Run0QueueState = Arc::new(std::sync::Mutex::new(Default::default()));
    builder.globals(move |scope| {
      scope.insert::<IpcSourcemap>(ipcmap.clone());
      scope.insert::<Run0QueueState>(run0_queue.clone());
    });

    Ok(())
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    Ok(())
  }
}
