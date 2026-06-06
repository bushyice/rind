// TODO: Reload @static

use std::path::Path;

use rind_core::prelude::*;

use rind_flow::transport::{
  TransportMethod, TransportProtocolId, TransportRoute, TransportRouteMetadata,
};
use rind_flow::{FlowFacet, FlowFacetMetadata, FlowImpulse, FlowImpulseMetadata};
use rind_ipc::FlowPayloadType;
use rind_primitives::mounts::Mount;
use rind_primitives::prelude::{Permission, Variable};
use rind_primitives::scopes::ScopeStore;
use rind_services::services::Service;
use rind_services::sockets::Socket;
use rind_services::timers::Timer;

use std::collections::HashMap;

use crate::loader::load_units_from;

fn add_builtin_defs(metadata: &mut Metadata) {
  metadata
    .group("rind")
    // Transports
    .insert::<TransportRoute>(TransportRouteMetadata {
      name: "sys-uds".into(),
      protocol: TransportMethod::Options {
        id: TransportProtocolId("uds".into()),
        options: vec![],
        permissions: Some(vec!["SystemServices".into()]),
      },
    })
    .insert::<TransportRoute>(TransportRouteMetadata {
      name: "sys-shm".into(),
      protocol: TransportMethod::Options {
        id: TransportProtocolId("shm".into()),
        options: vec![],
        permissions: Some(vec!["SystemServices".into()]),
      },
    })
    // Facets
    .insert::<FlowFacet>(FlowFacetMetadata {
      name: "up!".into(),
      payload: FlowPayloadType::None,
      ..Default::default()
    })
    .insert::<FlowFacet>(FlowFacetMetadata {
      name: "active".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowFacet>(FlowFacetMetadata {
      name: "inactive".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowFacet>(FlowFacetMetadata {
      name: "suspended".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowFacet>(FlowFacetMetadata {
      name: "user_session".into(),
      payload: FlowPayloadType::Json,
      branch: Some(vec!["session_id".into()]),
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    // Impulses
    .insert::<FlowImpulse>(FlowImpulseMetadata {
      name: "activate".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowImpulse>(FlowImpulseMetadata {
      name: "deactivate".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowImpulse>(FlowImpulseMetadata {
      name: "request_login".into(),
      payload: FlowPayloadType::Json,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowImpulse>(FlowImpulseMetadata {
      name: "request_logout".into(),
      payload: FlowPayloadType::Json,
      ..Default::default()
    })
    .insert::<FlowImpulse>(FlowImpulseMetadata {
      name: "ready".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .insert::<FlowImpulse>(FlowImpulseMetadata {
      name: "boot".into(),
      payload: FlowPayloadType::String,
      subscribers: Some(vec![
        TransportMethod::Type(TransportProtocolId("route:rind:sys-uds".into())),
        TransportMethod::Type(TransportProtocolId("route:rind:sys-shm".into())),
      ]),
      ..Default::default()
    })
    .close();
}

pub fn create_units_metadata<P: AsRef<Path>>(
  scope: &str,
  ctx: &mut OrchestratorContext<'_>,
  units_dir: P,
  permissions: Option<&PermissionStore>,
) -> CoreResult<Void> {
  let units_dir = units_dir.as_ref();

  let mut metadata = Metadata::new(scope)
    .of::<Service>("service")
    .of::<Timer>("timer")
    .of::<Mount>("mount")
    .of::<Socket>("socket")
    .of::<FlowFacet>("facet")
    .of::<FlowImpulse>("impulse")
    .of::<Permission>("permission")
    .of::<TransportRoute>("transport-route")
    .of::<Variable>("variable");

  metadata = EXTENSIONS.with(|extensions| {
    extensions
      .get()
      .ok_or(CoreError::InvalidState(
        "extension manager not initialized".into(),
      ))?
      .resolve("component", metadata)
  })?;

  if scope == "static" {
    add_builtin_defs(&mut metadata);

    metadata = EXTENSIONS.with(|extensions| {
      extensions
        .get()
        .ok_or(CoreError::InvalidState(
          "extension manager not initialized".into(),
        ))?
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

        Ok(Void)
      }
    }),
  )?;

  EXTENSIONS.with(|extensions| {
    extensions
      .get()
      .ok_or(CoreError::InvalidState(
        "extension manager not initialized".into(),
      ))?
      .act("loaded_units_scope", &mut metadata)
  })?;

  ctx.metadata.insert_metadata(metadata);
  build_indexes(ctx, scope)?;

  Ok(Void)
}

pub fn build_indexes(ctx: &mut OrchestratorContext<'_>, scope: &str) -> CoreResult<Void> {
  ctx.metadata.ensure_index_for_type::<Service>(scope)?;
  ctx.metadata.ensure_index_for_type::<Mount>(scope)?;
  ctx.metadata.ensure_index_for_type::<Socket>(scope)?;
  ctx.metadata.ensure_index_for_type::<Timer>(scope)?;
  ctx.metadata.ensure_index_for_type::<FlowFacet>(scope)?;
  ctx.metadata.ensure_index_for_type::<FlowImpulse>(scope)?;

  EXTENSIONS.with(|extensions| {
    extensions
      .get()
      .ok_or(CoreError::InvalidState(
        "extension manager not initialized".into(),
      ))?
      .act("create_index", ctx.metadata)
  })?;

  Ok(Void)
}

pub fn create_dynamic_scope<P: AsRef<Path>>(
  scope: impl Into<Ustr>,
  lifetime_state: Option<Ustr>,
  attributes: HashMap<Ustr, String>,
  ctx: &mut OrchestratorContext<'_>,
  units_dir: P,
) -> CoreResult<Void> {
  let scope = scope.into();
  create_units_metadata(scope.as_str(), ctx, units_dir, None)?;
  ScopeStore::upsert_global(scope, attributes, lifetime_state);
  Ok(Void)
}

pub fn destroy_dynamic_scope(scope: &str, ctx: &mut OrchestratorContext<'_>) -> CoreResult<Void> {
  if scope == "static" {
    return Ok(Void);
  }
  ctx.metadata.remove_metadata(scope);
  let _ = ScopeStore::remove_scope_global(scope);
  Ok(Void)
}
