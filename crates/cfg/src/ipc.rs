use rind_ipc::payloads::{ListPayload, SSPayload};
use rind_ipc::recv::IpcSourcemap;
use rind_ipc::ser::{
  FacetSerialized, ImpulseSerialized, IpcListComponent, IpcListPrinter, SerializeSerialized,
  SocketSerialized,
};
use rind_ipc::{Message, MessageType};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::user::{handle_ipc_login, handle_ipc_logout, handle_ipc_run0};
use rind_core::prelude::*;
use rind_core::reexports::*;
use rind_core::types::Ustr;
use rind_flow::{FacetGraph, FlowFacet, FlowImpulse};
use rind_ipc::payloads::{ScopeCreatePayload, ScopeDestroyPayload};
use rind_ipc::ser::{
  MountSerialized, ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many,
};
use rind_primitives::mounts::{Mount, is_mounted};
use rind_primitives::permissions::PERM_LOGIN;
use rind_primitives::permissions::{
  handle_ipc_grant_permission, handle_ipc_revoke_permission, handle_ipc_show_permission,
};
use rind_primitives::scopes::ScopeStore;
use rind_primitives::variables::VariableHeap;
use rind_services::sockets::{Socket, handle_ipc_start_socket, handle_ipc_stop_socket};
use rind_services::{Service, handle_ipc_start, handle_ipc_stop};

pub const IPC_RUNTIME_ID: &str = "ipc";

type IpcRequest = (Message, Sender<Message>);

pub struct IpcRuntime {
  incoming_tx: Sender<IpcRequest>,
  incoming_rx: Arc<Mutex<Receiver<IpcRequest>>>,
  listener_thread: Option<thread::JoinHandle<Void>>,
}

impl Default for IpcRuntime {
  fn default() -> Self {
    let (tx, rx) = mpsc::channel();
    Self {
      incoming_tx: tx,
      incoming_rx: Arc::new(Mutex::new(rx)),
      listener_thread: None,
    }
  }
}

pub fn payload_to<T: serde::de::DeserializeOwned + 'static>(
  mut payload: RuntimePayload,
) -> Result<T, CoreError> {
  let msg = payload.get::<Message>("message")?;
  msg.parse_payload::<T>().map_err(|x| CoreError::Custom(x))
}

pub fn payload_msg(mut payload: RuntimePayload) -> Result<Message, CoreError> {
  payload.get::<Message>("message")
}

fn build_ipc_list_response(
  payload: ListPayload,
  ctx: &mut RuntimeContext<'_>,
) -> Result<Message, CoreError> {
  let sm = ctx
    .registry
    .singleton::<FacetGraph>(FacetGraph::KEY)
    .ok_or_else(|| CoreError::InvalidState("state machine store not found".into()))?;

  Ok(if payload.unit_type == "unit" {
    let target_group = Ustr::from(payload.name.as_str());
    let mut services = Vec::new();
    let mut mounts = Vec::new();
    let mut sockets = Vec::new();
    let mut facets = Vec::new();
    let mut impulses = Vec::new();
    for (scope, items) in ctx.registry.metadata.all_items::<Service>() {
      for (group, meta) in items {
        if group == target_group {
          services.push((scope.clone(), meta));
        }
      }
    }
    for (scope, items) in ctx.registry.metadata.all_items::<Mount>() {
      for (group, meta) in items {
        if group == target_group {
          mounts.push((scope.clone(), meta));
        }
      }
    }
    for (scope, items) in ctx.registry.metadata.all_items::<Socket>() {
      for (group, meta) in items {
        if group == target_group {
          sockets.push((scope.clone(), meta));
        }
      }
    }
    for (scope, items) in ctx.registry.metadata.all_items::<FlowFacet>() {
      for (group, meta) in items {
        if group == target_group {
          facets.push((scope.clone(), meta));
        }
      }
    }
    for (scope, items) in ctx.registry.metadata.all_items::<FlowImpulse>() {
      for (group, meta) in items {
        if group == target_group {
          impulses.push((scope.clone(), meta));
        }
      }
    }

    let ser_instances: HashMap<Ustr, (String, Vec<u32>)> = services
      .iter()
      .filter_map(|(scope, ser)| {
        let scoped = Ustr::from(format!("{}:{}@{}", payload.name, ser.name, scope));
        ctx.registry.as_one::<Service>("*", scoped).ok().map(|x| {
          (
            ser.name.clone(),
            (x.instances.last_state(), x.instances.pid()),
          )
        })
      })
      .collect();

    Message::from_type(MessageType::Ok).with(
      UnitItemsSerialized {
        mounts: mounts
          .iter()
          .map(|(_, mnt)| MountSerialized {
            fstype: mnt.fstype.clone(),
            mounted: is_mounted(&mnt.target).unwrap_or(false),
            source: mnt.source.clone(),
            target: mnt.target.clone(),
          })
          .collect(),
        services: services
          .iter()
          .map(|(_, svc)| ServiceSerialized {
            after: svc.after.clone(),
            run: svc.run.as_many().map(|x| x.exec.clone()).collect(),
            last_state: ser_instances
              .get(svc.name())
              .map_or("Inactive".to_string(), |x| x.0.clone()),
            name: svc.name().into(),
            pid: ser_instances
              .get(svc.name())
              .map_or(None, |x| if x.1.is_empty() { None } else { Some(x.1[0]) }),
            restart: svc.restart.as_ref().map_or(false, |_| true),
          })
          .collect(),
        sockets: sockets
          .iter()
          .map(|(scope, x)| SocketSerialized {
            name: x.name.clone(),
            active: ctx
              .registry
              .as_one::<Socket>(
                "*",
                Ustr::from(format!("{}:{}@{}", payload.name, x.name, scope)),
              )
              .is_ok(),
            listen: x.listen.clone(),
            triggers: x.trigger.as_ref().map_or(0, |x| x.len()),
            r#type: format!("{:?}", x.r#type).to_ustr(),
          })
          .collect(),
        facets: facets
          .iter()
          .map(|(scope, st)| FacetSerialized {
            name: st.name.clone(),
            instances: sm
              .facets
              .get(&Ustr::from(format!(
                "{}:{}@{}",
                payload.name, st.name, scope
              )))
              .or_else(|| {
                sm.facets
                  .get(&Ustr::from(format!("{}:{}", payload.name, st.name)))
              })
              .map_or(Default::default(), |x| {
                x.iter()
                  .map(|x| flexbuffers::to_vec(&x.payload.to_json()).unwrap_or_default())
                  .collect()
              }),
            keys: st.branch.clone().unwrap_or_default(),
          })
          .collect(),
        impulses: impulses
          .iter()
          .map(|(_, st)| ImpulseSerialized {
            name: st.name.clone(),
          })
          .collect(),
      }
      .serialize(),
    )
  } else if payload.unit_type == "service" {
    let Some(service_meta) = ctx
      .registry
      .metadata
      .find::<Service>("*", payload.name.clone())
    else {
      return Err(CoreError::MetadataNotFound(format!(
        "Service not found: {}",
        payload.name
      )));
    };

    let service = if let Ok(s) = ctx
      .registry
      .as_one::<Service>("*", Ustr::from(payload.name.as_str()))
    {
      s
    } else {
      &Service::new(service_meta)
    };

    Message::from_type(MessageType::Ok).with(
      ServiceSerialized {
        name: service.metadata.name.clone(),
        after: service.metadata.after.clone(),
        last_state: service.instances.last_state(),
        pid: service.instances.pid().get(0).cloned(),
        restart: service.metadata.restart.as_ref().map_or(false, |_| true),
        run: service
          .metadata
          .run
          .as_many()
          .map(|x| x.exec.clone())
          .collect(),
      }
      .serialize(),
    )
  } else if payload.unit_type == "socket" {
    let Some(sock_meta) = ctx
      .registry
      .metadata
      .find::<Socket>("*", payload.name.clone())
    else {
      return Err(CoreError::MetadataNotFound(format!(
        "Socket not found: {}",
        payload.name
      )));
    };

    let active = ctx
      .registry
      .as_one::<Socket>("*", Ustr::from(payload.name.as_str()))
      .map_or(false, |x| x.active);

    Message::from_type(MessageType::Ok).with(
      SocketSerialized {
        name: sock_meta.name.clone(),
        active,
        listen: sock_meta.listen.clone(),
        triggers: sock_meta.trigger.as_ref().map_or(0, |x| x.len()),
        r#type: format!("{:?}", sock_meta.r#type).to_ustr(),
      }
      .serialize(),
    )
  } else if payload.unit_type == "facet" && !payload.name.is_empty() {
    let name_ustr = Ustr::from(payload.name.as_str());
    let instances = sm.facets.get(&name_ustr);
    let Some(def) = ctx
      .registry
      .metadata
      .find::<FlowFacet>("*", payload.name.clone())
    else {
      return Err(CoreError::MetadataNotFound(format!(
        "Facet not found: {}",
        payload.name
      )));
    };
    let branches = def.branch.as_ref();

    Message::from_type(MessageType::Ok).with(
      FacetSerialized {
        name: payload.name,
        instances: instances.map_or(Default::default(), |x| {
          x.iter()
            .map(|x| flexbuffers::to_vec(&x.payload.to_json()).unwrap_or_default())
            .collect()
        }),
        keys: if let Some(branches) = branches {
          branches.clone()
        } else {
          Default::default()
        },
      }
      .serialize(),
    )
  } else if payload.unit_type == "facet" {
    let facets = &sm.facets;

    Message::from_type(MessageType::Ok).with(
      flexbuffers::to_vec(
        &facets
          .iter()
          .filter_map(|(name, inst)| {
            let def = ctx
              .registry
              .metadata
              .find::<FlowFacet>("*", name.as_str())?;
            let branches = def.branch.as_ref()?;
            Some(FacetSerialized {
              name: name.clone(),
              instances: inst
                .iter()
                .map(|x| flexbuffers::to_vec(&x.payload.to_json()).unwrap_or_default())
                .collect(),
              keys: branches.clone(),
            })
          })
          .collect::<Vec<FacetSerialized>>(),
      )
      .unwrap_or_default(),
    )
  } else if payload.unit_type == "unknown"
    || payload.unit_type == "units"
    || payload.unit_type == "all"
    || payload.unit_type.is_empty()
  {
    let mut units_map: HashMap<Ustr, UnitSerialized> = HashMap::new();

    if let Some(groups) = ctx.registry.metadata.groups("*") {
      for group in groups {
        let mut services = Vec::new();
        for (scope, items) in ctx.registry.metadata.all_items::<Service>() {
          for (g, s) in items {
            if g == group {
              services.push((scope.clone(), s));
            }
          }
        }
        let mounts = if let Some(mounts) = ctx
          .registry
          .metadata
          .group_items::<Mount>("*", group.clone())
        {
          mounts
        } else {
          Vec::new()
        };
        let mut sockets = Vec::new();
        for (scope, items) in ctx.registry.metadata.all_items::<Socket>() {
          for (g, s) in items {
            if g == group {
              sockets.push((scope.clone(), s));
            }
          }
        }
        let mut facets = Vec::new();
        for (scope, items) in ctx.registry.metadata.all_items::<FlowFacet>() {
          for (g, s) in items {
            if g == group {
              facets.push((scope.clone(), s));
            }
          }
        }

        let impulses = if let Some(impulses) = ctx
          .registry
          .metadata
          .group_items::<FlowImpulse>("*", group.clone())
        {
          impulses
        } else {
          Vec::new()
        };

        let mounted = mounts
          .iter()
          .filter(|mnt| is_mounted(&mnt.target).unwrap_or(false))
          .count();
        let active_services = services
          .iter()
          .filter(|(scope, s)| {
            ctx
              .registry
              .instances::<Service>("*", Ustr::from(format!("{group}:{}@{}", s.name, scope)))
              .ok()
              .map_or(false, |x| x.iter().any(|x| x.instances.is_active()))
          })
          .count();
        let active_sockets = sockets
          .iter()
          .filter(|(scope, s)| {
            ctx
              .registry
              .instances::<Socket>("*", Ustr::from(format!("{group}:{}@{}", s.name, scope)))
              .ok()
              .map_or(false, |x| x.iter().any(|x| x.active))
          })
          .count();
        let active_facets = facets
          .iter()
          .filter(|(scope, s)| {
            sm.facets
              .get(&Ustr::from(format!("{group}:{}@{}", s.name, scope)))
              .or_else(|| sm.facets.get(&Ustr::from(format!("{group}:{}", s.name))))
              .is_some()
          })
          .count();

        units_map.insert(
          group.clone(),
          UnitSerialized {
            active_services,
            active_sockets,
            active_facets,
            mounted: mounted,
            mounts: mounts.len(),
            name: group.clone(),
            services: services.len(),
            sockets: sockets.len(),
            facets: facets.len(),
            impulses: impulses.len(),
          },
        );
      }
    }

    let mut units_list: Vec<UnitSerialized> = units_map.into_values().collect();
    units_list.sort_by(|a, b| a.name.cmp(&b.name));
    Message::from_type(MessageType::Ok).with(serialize_many(&units_list))
  } else {
    let msg = EXTENSIONS
      .with(|extensions| {
        extensions
          .get()
          .expect("extension manager not initialized")
          .resolve(
            &format!("ipc:list:{}", payload.unit_type),
            ExtensionExecutionCtx::new(IpcListComponent::default()),
          )
      })?
      .dispatch(None, None, Some(&mut ctx.registry))?
      .downcast::<IpcListComponent>()
      .map_err(|_| CoreError::Unknown)?;
    Message::from_type(MessageType::Ok).with(msg.serialize())
  })
}

pub fn handle_ipc_list(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let payload = msg
    .parse_payload::<ListPayload>()
    .map_err(CoreError::Custom)?;
  Ok(
    build_ipc_list_response(payload, ctx)
      .or_else(|x| {
        Ok::<Message, Message>(Message::from_type(MessageType::Error).with_string(x.to_string()))
      })
      .unwrap(),
  )
}

pub fn handle_ipc_set(
  _msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let _ = ctx
    .registry
    .singleton::<VariableHeap>(VariableHeap::KEY)
    .ok_or(CoreError::Custom("Failed to get variable heap".into()))?;

  Ok(Message::default())
}

pub fn handle_ipc_life(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  log: &LogHandle,
  action: &str,
) -> Result<Message, CoreError> {
  let payload = msg
    .parse_payload::<SSPayload>()
    .map_err(CoreError::Custom)?;

  let message = EXTENSIONS
    .with(|extensions| {
      extensions
        .get()
        .expect("extension manager not initialized")
        .resolve(
          &format!("ipc:{action}:{}", payload.unit_type),
          ExtensionExecutionCtx::new(payload),
        )
    })?
    .dispatch(Some(dispatch), Some(log), Some(&mut ctx.registry))?
    .downcast::<Message>()
    .map_err(|_| Message::default());

  Ok(match message {
    Ok(msg) => *msg,
    Err(msg) => msg,
  })
}

pub fn handle_ipc_start_unknown(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  log: &LogHandle,
) -> Result<Message, CoreError> {
  handle_ipc_life(msg, ctx, dispatch, log, "start")
}

pub fn handle_ipc_stop_unknown(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  log: &LogHandle,
) -> Result<Message, CoreError> {
  handle_ipc_life(msg, ctx, dispatch, log, "stop")
}

pub fn handle_ipc_remove(
  _msg: Message,
  _ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  Ok(Message::default())
}

pub fn handle_ipc_create_scope(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  if msg.from_uid.unwrap_or(u32::MAX) != 0 {
    return Err(CoreError::PermissionDenied);
  }

  let payload = msg
    .parse_payload::<ScopeCreatePayload>()
    .map_err(CoreError::Custom)?;
  let scope = payload.scope.trim();
  if scope.is_empty() || scope == "static" {
    return Err(CoreError::Custom("invalid scope name".into()));
  }

  let mut attrs = std::collections::HashMap::new();
  for (k, v) in payload.attributes {
    attrs.insert(Ustr::from(k), v);
  }

  let lifetime_state = payload.lifetime_state.clone().map(Ustr::from);
  ScopeStore::upsert_global(scope, attrs.clone(), lifetime_state.clone());
  ScopeStore::desired_scope_upsert(scope, attrs.clone(), lifetime_state.clone());

  if let Some(store) = ctx.registry.singleton_mut::<ScopeStore>(ScopeStore::KEY) {
    store.upsert(scope, attrs, lifetime_state.clone());
  }

  if let Some(sm) = ctx.registry.singleton_mut::<FacetGraph>(FacetGraph::KEY) {
    let _ = sm.load_scope_from_persistence(scope);
  }
  ctx.lifecycle.request(LifecycleAction::ReloadUnits);

  Ok(Message::ok("scope created"))
}

pub fn handle_ipc_destroy_scope(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  if msg.from_uid.unwrap_or(u32::MAX) != 0 {
    return Err(CoreError::PermissionDenied);
  }

  let payload = msg
    .parse_payload::<ScopeDestroyPayload>()
    .map_err(CoreError::Custom)?;
  let scope = payload.scope.trim();
  if scope.is_empty() || scope == "static" {
    return Err(CoreError::Custom("invalid scope name".into()));
  }

  let _ = ScopeStore::remove_scope_global(scope);
  ScopeStore::desired_scope_remove(scope);
  if let Some(store) = ctx.registry.singleton_mut::<ScopeStore>(ScopeStore::KEY) {
    let _ = store.remove_scope(scope);
  }
  if let Some(sm) = ctx.registry.singleton_mut::<FacetGraph>(FacetGraph::KEY) {
    let _ = sm.drop_scope(scope);
  }
  ctx.lifecycle.request(LifecycleAction::ReloadUnits);

  Ok(Message::ok("scope destroyed"))
}

pub fn handle_ipc_list_scopes(
  _msg: Message,
  _ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  let mut list = IpcListComponent::default().with_printer(IpcListPrinter {
    r#type: "list".to_string(),
    titles: vec!["Name".to_string(), "Attributes".to_string()],
    keys: vec!["name".to_string(), "attributes".to_string()],
    colors: vec!["blue".to_string(), "yellow".to_string()],
  });

  for s in ScopeStore::list_global() {
    list.add(s);
  }

  Ok(Message::from_type(MessageType::Ok).with(flexbuffers::to_vec(&list).unwrap_or_default()))
}

fn queue_lifecycle_action(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  action: LifecycleAction,
  response: &str,
) -> Result<Message, CoreError> {
  if msg.from_uid.unwrap_or(u32::MAX) != 0 {
    return Err(CoreError::PermissionDenied);
  }

  ctx.lifecycle.request(action);
  Ok(Message::ok(response))
}

pub fn handle_ipc_reload_units(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  queue_lifecycle_action(
    msg,
    ctx,
    LifecycleAction::ReloadUnits,
    "unit reload scheduled",
  )
}

pub fn handle_ipc_reboot(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  queue_lifecycle_action(msg, ctx, LifecycleAction::Reboot, "reboot scheduled")
}

pub fn handle_ipc_soft_reboot(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  queue_lifecycle_action(
    msg,
    ctx,
    LifecycleAction::SoftReboot,
    "soft reboot scheduled",
  )
}

pub fn handle_ipc_shutdown(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  queue_lifecycle_action(msg, ctx, LifecycleAction::Shutdown, "shutdown scheduled")
}

#[runtime("ipc")]
impl IpcRuntime {
  fn init_actions(&mut self) {
    let ipcsrc = ctx.scope.get::<IpcSourcemap>().cloned().unwrap_or_default();
    ipcsrc.register("login", handle_ipc_login, PERM_LOGIN);
    ipcsrc.register("logout", handle_ipc_logout, PermissionExpr::All);
    ipcsrc.register("run0", handle_ipc_run0, PermissionExpr::All);
    ipcsrc.register("start_service", handle_ipc_start, PermissionExpr::All);
    ipcsrc.register("stop_service", handle_ipc_stop, PermissionExpr::All);
    ipcsrc.register("start_socket", handle_ipc_start_socket, PermissionExpr::All);
    ipcsrc.register("stop_socket", handle_ipc_stop_socket, PermissionExpr::All);
    ipcsrc.register("start", handle_ipc_start_unknown, PermissionExpr::All);
    ipcsrc.register("stop", handle_ipc_stop_unknown, PermissionExpr::All);
    ipcsrc.register("list", handle_ipc_list, PermissionExpr::All);
    ipcsrc.register("set_variable", handle_ipc_set, PermissionExpr::All);
    ipcsrc.register("remove_variable", handle_ipc_remove, PermissionExpr::All);
    ipcsrc.register("create_scope", handle_ipc_create_scope, PermissionExpr::All);
    ipcsrc.register(
      "destroy_scope",
      handle_ipc_destroy_scope,
      PermissionExpr::All,
    );
    ipcsrc.register(
      "show_permissions",
      handle_ipc_show_permission,
      PermissionExpr::All,
    );
    ipcsrc.register(
      "grant_permission",
      handle_ipc_grant_permission,
      PermissionExpr::RootOnly,
    );
    ipcsrc.register(
      "revoke_permission",
      handle_ipc_revoke_permission,
      PermissionExpr::RootOnly,
    );
    ipcsrc.register("reload_units", handle_ipc_reload_units, PermissionExpr::All);
    ipcsrc.register("reboot", handle_ipc_reboot, PermissionExpr::All);
    ipcsrc.register("soft_reboot", handle_ipc_soft_reboot, PermissionExpr::All);
    ipcsrc.register("shutdown", handle_ipc_shutdown, PermissionExpr::All);
  }

  #[action()]
  fn start_server(&mut self) {
    if self.listener_thread.is_none() {
      let tx = self.incoming_tx.clone();
      let notifier = ctx.notifier.clone();
      self.listener_thread = Some(thread::spawn(move || {
        let socket_path = std::env::var("RIND_SOC_PATH")
          .map(PathBuf::from)
          .unwrap_or_else(|_| PathBuf::from("/tmp/rind.sock"));
        let _ = std::fs::remove_file(&socket_path);
        let listener = match UnixListener::bind(&socket_path) {
          Ok(l) => l,
          Err(e) => {
            // TODO: i'll use log instead
            eprintln!("[ipc] failed to bind {:?}: {}", socket_path, e);
            return;
          }
        };

        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o666))
          .expect("failed to allow permissions");

        for stream in listener.incoming() {
          if let Ok(stream) = stream {
            let tx = tx.clone();
            let notifier = notifier.clone();
            thread::spawn(move || {
              handle_client_connection(stream, tx, notifier);
            });
          }
        }
      }));
    }
  }

  fn drain_requests(&mut self) {
    if let Ok(rx) = self.incoming_rx.lock() {
      while let Ok((msg, reply_tx)) = rx.try_recv() {
        let response = handle_ipc_message(msg, ctx, dispatch, log);
        let _ = reply_tx.send(response);
      }
    }
  }
}

use rind_core::notifier::Notifier;

fn handle_client_connection(
  mut stream: UnixStream,
  parent_tx: Sender<IpcRequest>,
  notifier: Option<Notifier>,
) {
  let cred = get_peer_cred_stream(&stream).expect("failed to get cred");
  loop {
    let msg = match Message::read_signed(&mut stream) {
      Ok(m) => {
        if cred.uid == 0 && m.from_uid.is_some() {
          m
        } else {
          m.from_gid(cred.gid).from_uid(cred.uid).from_pid(cred.pid)
        }
      }
      Err(_) => break,
    };

    let (reply_tx, reply_rx) = mpsc::channel::<Message>();
    if parent_tx.send((msg, reply_tx)).is_err() {
      break;
    }
    if let Some(notif) = &notifier {
      let _ = notif.notify();
    }

    let response: Message = match reply_rx.recv() {
      Ok(resp) => resp,
      Err(_) => break,
    };

    if response.write_signed(&mut stream).is_err() {
      break;
    }
  }
}

fn handle_ipc_message(
  msg: Message,
  ctx: &mut RuntimeContext<'_>,
  dispatch: &RuntimeDispatcher,
  log: &LogHandle,
) -> Message {
  let pm = ctx
    .registry
    .singleton::<PermissionStore>(PermissionStore::KEY)
    .cloned()
    .unwrap_or_default();

  let ipcsrc_shared = ctx.scope.get::<IpcSourcemap>().cloned().unwrap_or_default();

  let Some(source) = ipcsrc_shared.message(&msg.action) else {
    return Message::from_type(MessageType::Error)
      .with_string(format!("Message handler not found: {:?}", msg.action));
  };

  drop(ipcsrc_shared);

  if !matches!(&source.perms, PermissionExpr::All)
    && !pm.user_check(msg.from_uid.unwrap_or(0), &source.perms)
  {
    return Message::from_type(MessageType::Error).with_string(format!("Permission Denied"));
  }

  let mut fields = HashMap::new();
  fields.insert("name".to_string(), msg.action.clone());
  log.log(LogLevel::Trace, "ipc-runtime", "ipc call", fields);

  match (source.handler)(msg, ctx, dispatch, log) {
    Ok(resp) => resp,
    Err(e) => {
      Message::from_type(MessageType::Error).with_string(format!("IPC handler failed: {e}"))
    }
  }
}
