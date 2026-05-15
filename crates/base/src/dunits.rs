use std::fs;
use std::path::Path;

use rind_core::prelude::*;

use crate::{
  flow::{Signal, State},
  loader::load_units_from,
  mount::Mount,
  prelude::{Permission, Service, Variable},
  scopes::ScopeStore,
  sockets::Socket,
  timers::Timer,
};

use std::collections::HashMap;

fn add_builtin_defs(metadata: &mut Metadata) {
  // TODO: Switch from toml into a declarative method.
  let builtin_toml = r#"
[[state]]
name = "active"
payload = "string"

[[state]]
name = "inactive"
payload = "string"

[[state]]
name = "suspended"
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

[[signal]]
name = "boot"
payload = "string"

"#;

  let _ = metadata.from_toml(builtin_toml, "rind");
}

pub fn create_units_metadata<P: AsRef<Path>>(
  scope: &str,
  ctx: &mut OrchestratorContext<'_>,
  units_dir: P,
  permissions: Option<&PermissionStore>,
) -> CoreResult<()> {
  let units_dir = units_dir.as_ref();

  let mut metadata = Metadata::new(scope)
    .of::<Service>("service")
    .of::<Timer>("timer")
    .of::<Mount>("mount")
    .of::<Socket>("socket")
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

  if scope == "static" {
    add_builtin_defs(&mut metadata);

    metadata = EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .resolve("built_in", metadata)
    })?;
  }

  load_units_from(
    ctx,
    &mut metadata,
    &units_dir,
    permissions.map(|permissions| {
      |content: &str, group: &Ustr, metadata: &mut Metadata| {
        if content.contains("permission") {
          if let Some(items) = metadata.get_in_group::<Permission>(group.clone()) {
            for perm in items {
              permissions.reg_perm(PermissionId(perm.id), perm.name.clone())?;
            }
          }
        }

        Ok(())
      }
    }),
  )?;

  EXTENSIONS.with(|extensions| {
    extensions
      .get()
      .expect("extension manager not initialized")
      .act("loaded_units_scope", &mut metadata)
  })?;

  ctx.metadata.insert_metadata(metadata);
  ctx.metadata.ensure_index_for_type::<Service>(scope)?;
  ctx.metadata.ensure_index_for_type::<Mount>(scope)?;
  ctx.metadata.ensure_index_for_type::<Socket>(scope)?;
  ctx.metadata.ensure_index_for_type::<Timer>(scope)?;
  ctx.metadata.ensure_index_for_type::<State>(scope)?;
  ctx.metadata.ensure_index_for_type::<Signal>(scope)?;

  EXTENSIONS.with(|extensions| {
    extensions
      .get()
      .expect("extension manager not initialized")
      .act("create_index", ctx.metadata)
  })?;

  Ok(())
}

pub fn create_dynamic_scope<P: AsRef<Path>>(
  scope: impl Into<Ustr>,
  lifetime_state: Option<Ustr>,
  attributes: HashMap<Ustr, serde_json::Value>,
  ctx: &mut OrchestratorContext<'_>,
  units_dir: P,
) -> CoreResult<()> {
  let scope = scope.into();
  create_units_metadata(scope.as_str(), ctx, units_dir, None)?;
  ScopeStore::upsert_global(scope, attributes, lifetime_state);
  Ok(())
}

pub fn destroy_dynamic_scope(scope: &str, ctx: &mut OrchestratorContext<'_>) -> CoreResult<()> {
  if scope == "static" {
    return Ok(());
  }
  ctx.metadata.remove_metadata(scope);
  let _ = ScopeStore::remove_scope_global(scope);
  Ok(())
}

pub fn create_scope_metadata_runtime<P: AsRef<Path>>(
  scope: impl Into<Ustr>,
  metadata_registry: &mut MetadataRegistry,
  units_dir: P,
) -> CoreResult<()> {
  let scope = scope.into();
  let mut metadata = Metadata::new(scope.clone())
    .of::<Service>("service")
    .of::<Timer>("timer")
    .of::<Mount>("mount")
    .of::<Socket>("socket")
    .of::<State>("state")
    .of::<Signal>("signal")
    .of::<Permission>("permission")
    .of::<Variable>("variable");

  let dir = units_dir.as_ref();
  if let Ok(entries) = fs::read_dir(dir) {
    for entry in entries.flatten() {
      let path = entry.path();
      if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
        continue;
      }
      if path.extension().and_then(|x| x.to_str()) != Some("toml") {
        continue;
      }
      let group = Ustr::from(
        path
          .file_stem()
          .and_then(|s| s.to_str())
          .unwrap_or("unknown"),
      );
      if let Ok(content) = fs::read_to_string(&path) {
        let _ = metadata.from_toml(&content, group);
      }
    }
  }

  metadata_registry.insert_metadata(metadata);
  let _ = metadata_registry.ensure_index_for_type::<Service>(scope.clone());
  let _ = metadata_registry.ensure_index_for_type::<Mount>(scope.clone());
  let _ = metadata_registry.ensure_index_for_type::<Socket>(scope.clone());
  let _ = metadata_registry.ensure_index_for_type::<Timer>(scope.clone());
  let _ = metadata_registry.ensure_index_for_type::<State>(scope.clone());
  let _ = metadata_registry.ensure_index_for_type::<Signal>(scope);
  Ok(())
}
