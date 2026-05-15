use std::path::PathBuf;
use std::sync::Arc;

use crate::dunits::create_units_metadata;
use crate::flow::{StateMachine, state_scope_path};
use crate::loader::RegisterLoader;
use crate::permissions::{PERM_LOGIN, PERM_RUN0, PERM_SYSTEM_SERVICES};
use crate::scopes::ScopeStore;
use crate::user::Run0QueueState;
use crate::variables::{Variable, VariableHeap, variables_path};
use rind_core::prelude::*;
use rind_core::user::{PamHandle, UserStore};
use rind_ipc::recv::IpcSourcemap;

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
    create_units_metadata("static", ctx, &self.units_dir, Some(permissions))
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
    EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .act("register", &mut RegisterLoader)
    })?;

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

    if ctx.metadata.metadata("static").is_none() {
      self.load_all_units(ctx, &permissions)?;
      self.load_permissions(&permissions)?;
    }

    for spec in ScopeStore::desired_scopes() {
      if spec.name.as_str() == "static" {
        continue;
      }
      let scope_units_dir = spec
        .attributes
        .get(&Ustr::from("units_dir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| self.units_dir.clone());
      if ctx.metadata.metadata(spec.name.clone()).is_none() {
        let _ = create_units_metadata(
          spec.name.as_str(),
          ctx,
          &scope_units_dir,
          Some(&permissions),
        );
      }
      ScopeStore::upsert_global(
        spec.name.clone(),
        spec.attributes.clone(),
        spec.lifetime_state.clone(),
      );
    }

    let metadata = &*ctx.metadata;
    let users = self.users.clone();
    ctx
      .runtime
      .with_instances(|instances| -> std::result::Result<(), CoreError> {
        let mut registry = InstanceRegistry::new(metadata, instances);

        let _ = registry.singleton_or_insert_with::<StateMachine>(StateMachine::KEY, || {
          let mut state =
            StateMachine::from_persistence(StatePersistence::new(state_scope_path("static")));
          let _ = state.load_from_persistence();
          let _ = state.save_all_scopes();
          state
        });
        let scopes = registry
          .singleton_or_insert_with::<ScopeStore>(ScopeStore::KEY, || ScopeStore::default());
        scopes.upsert("static", Default::default(), None);
        ScopeStore::upsert_global("static", Default::default(), None);

        let _ = registry.singleton_or_insert_with::<Arc<PamHandle>>(PamHandle::KEY, || {
          Arc::new(PamHandle::new(users.clone()))
        });

        let variable_heap =
          registry.singleton_or_insert_with::<VariableHeap>(VariableHeap::KEY, || {
            let mut heap = VariableHeap::new(variables_path());
            let _ = heap.load();
            heap
          });

        for meta_name in ctx.metadata.metadata_names() {
          let Some(units) = ctx.metadata.metadata(meta_name.clone()) else {
            continue;
          };
          for group in units.groups() {
            if let Some(vars) = ctx
              .metadata
              .group_items::<Variable>(meta_name.clone(), group)
            {
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
