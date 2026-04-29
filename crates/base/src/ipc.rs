use rind_ipc::payloads::ListPayload;
use rind_ipc::recv::IpcSourcemap;
use rind_ipc::ser::{NetworkStatusSerialized, SignalSerialized, SocketSerialized, StateSerialized};
use rind_ipc::{Message, MessageType};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::flow::{Signal, State, StateMachine};
use crate::mount::{Mount, is_mounted};
use crate::networking::{get_ports, handle_ipc_network};
use crate::permissions::{PERM_LOGIN, PERM_NETWORK};
use crate::services::{Service, handle_ipc_start, handle_ipc_stop};
use crate::sockets::{Socket, handle_ipc_start_socket, handle_ipc_stop_socket};
use crate::user::{handle_ipc_login, handle_ipc_logout, handle_ipc_run0};
use crate::variables::VariableHeap;
use rind_core::prelude::*;
use rind_ipc::ser::{
  MountSerialized, ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many,
};

pub const IPC_RUNTIME_ID: &str = "ipc";

type IpcRequest = (Message, Sender<Message>);

pub struct IpcRuntime {
  incoming_tx: Sender<IpcRequest>,
  incoming_rx: Arc<Mutex<Receiver<IpcRequest>>>,
  listener_thread: Option<thread::JoinHandle<()>>,
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
    .singleton::<StateMachine>(StateMachine::KEY)
    .ok_or_else(|| CoreError::InvalidState("state machine store not found".into()))?;

  Ok(if payload.unit_type == "unit" {
    let services = if let Some(services) = ctx
      .registry
      .metadata
      .group_items::<Service>("units", Ustr::from(payload.name.as_str()))
    {
      services
    } else {
      Vec::new()
    };
    let mounts = if let Some(mounts) = ctx
      .registry
      .metadata
      .group_items::<Mount>("units", Ustr::from(payload.name.as_str()))
    {
      mounts
    } else {
      Vec::new()
    };
    let sockets = if let Some(sockets) = ctx
      .registry
      .metadata
      .group_items::<Socket>("units", Ustr::from(payload.name.as_str()))
    {
      sockets
    } else {
      Vec::new()
    };
    let states = if let Some(states) = ctx
      .registry
      .metadata
      .group_items::<State>("units", Ustr::from(payload.name.as_str()))
    {
      states
    } else {
      Vec::new()
    };
    let signals = if let Some(signals) = ctx
      .registry
      .metadata
      .group_items::<Signal>("units", Ustr::from(payload.name.as_str()))
    {
      signals
    } else {
      Vec::new()
    };

    let ser_instances: HashMap<Ustr, (String, Vec<u32>)> = services
      .iter()
      .filter_map(|ser| {
        ctx
          .registry
          .as_one::<Service>(
            "units",
            Ustr::from(format!("{}@{}", payload.name, ser.name)),
          )
          .map_or(None, |x| {
            Some((
              ser.name.clone(),
              (x.instances.last_state(), x.instances.pid()),
            ))
          })
      })
      .collect();

    Message::from_type(MessageType::Ok).with(
      UnitItemsSerialized {
        mounts: mounts
          .iter()
          .map(|mnt| MountSerialized {
            fstype: mnt.fstype.clone(),
            mounted: is_mounted(&mnt.target).unwrap_or(false),
            source: mnt.source.clone(),
            target: mnt.target.clone(),
          })
          .collect(),
        services: services
          .iter()
          .map(|svc| ServiceSerialized {
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
          .map(|x| SocketSerialized {
            name: x.name.clone(),
            active: ctx
              .registry
              .as_one::<Socket>("units", Ustr::from(format!("{}@{}", payload.name, x.name)))
              .is_ok(),
            listen: x.listen.clone(),
            triggers: x.trigger.as_ref().map_or(0, |x| x.len()),
            r#type: format!("{:?}", x.r#type).to_ustr(),
          })
          .collect(),
        states: states
          .iter()
          .map(|st| StateSerialized {
            name: st.name.clone(),
            instances: sm
              .states
              .get(&Ustr::from(format!("{}@{}", payload.name, st.name)))
              .map_or(Default::default(), |x| {
                x.iter().map(|x| x.payload.to_json()).collect()
              }),
            keys: st.branch.clone().unwrap_or_default(),
          })
          .collect(),
        signals: signals
          .iter()
          .map(|st| SignalSerialized {
            name: st.name.clone(),
          })
          .collect(),
      }
      .stringify(),
    )
  } else if payload.unit_type == "service" {
    let Some(service_meta) = ctx
      .registry
      .metadata
      .find::<Service>("units", payload.name.clone())
    else {
      return Err(CoreError::MetadataNotFound(format!(
        "Service not found: {}",
        payload.name
      )));
    };

    let service = if let Ok(s) = ctx
      .registry
      .as_one::<Service>("units", Ustr::from(payload.name.as_str()))
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
      .stringify(),
    )
  } else if payload.unit_type == "socket" {
    let Some(sock_meta) = ctx
      .registry
      .metadata
      .find::<Socket>("units", payload.name.clone())
    else {
      return Err(CoreError::MetadataNotFound(format!(
        "Socket not found: {}",
        payload.name
      )));
    };

    let active = ctx
      .registry
      .as_one::<Socket>("units", Ustr::from(payload.name.as_str()))
      .map_or(false, |x| x.active);

    Message::from_type(MessageType::Ok).with(
      SocketSerialized {
        name: sock_meta.name.clone(),
        active,
        listen: sock_meta.listen.clone(),
        triggers: sock_meta.trigger.as_ref().map_or(0, |x| x.len()),
        r#type: format!("{:?}", sock_meta.r#type).to_ustr(),
      }
      .stringify(),
    )
  } else if payload.unit_type == "state" && !payload.name.is_empty() {
    let name_ustr = Ustr::from(payload.name.as_str());
    let instances = sm.states.get(&name_ustr);
    let Some(def) = ctx
      .registry
      .metadata
      .find::<State>("units", payload.name.clone())
    else {
      return Err(CoreError::MetadataNotFound(format!(
        "State not found: {}",
        payload.name
      )));
    };
    let branches = def.branch.as_ref();

    Message::from_type(MessageType::Ok).with(
      StateSerialized {
        name: payload.name,
        instances: instances.map_or(Default::default(), |x| {
          x.iter().map(|x| x.payload.to_json()).collect()
        }),
        keys: if let Some(branches) = branches {
          branches.clone()
        } else {
          Default::default()
        },
      }
      .stringify(),
    )
  } else if payload.unit_type == "state" {
    let states = &sm.states;

    Message::from_type(MessageType::Ok).with(
      serde_json::to_string(
        &states
          .iter()
          .filter_map(|(name, inst)| {
            let def = ctx
              .registry
              .metadata
              .find::<State>("units", name.as_str())?;
            let branches = def.branch.as_ref()?;
            Some(StateSerialized {
              name: name.clone(),
              instances: inst.iter().map(|x| x.payload.to_json()).collect(),
              keys: branches.clone(),
            })
          })
          .collect::<Vec<StateSerialized>>(),
      )
      .unwrap_or_default(),
    )
  } else if payload.unit_type == "netiface" {
    let mut statuses = Vec::new();
    if let Some(groups) = ctx.registry.metadata.groups("units") {
      for group in groups {
        if let Some(cfgs) = ctx
          .registry
          .metadata
          .group_items::<crate::networking::NetworkConfig>("units", group)
        {
          for cfg in cfgs {
            let config = {
              if let Some(instances) = sm.states.get(&Ustr::from("rind@net-configured")) {
                instances.iter().find(|i| {
                  if let Some(obj) = i.payload.to_json().as_object() {
                    obj.get("name").and_then(|v| v.as_str()) == Some(cfg.name.as_str())
                  } else {
                    false
                  }
                })
              } else {
                None
              }
            };
            let state = if config.is_some() {
              "Configured"
            } else {
              "Down"
            }
            .to_string();
            statuses.push(NetworkStatusSerialized {
              interface: cfg.name.clone().into(),
              method: match cfg.method {
                crate::networking::NetworkMethod::Dhcp => "dhcp".into(),
                crate::networking::NetworkMethod::Static => "static".into(),
              },
              address: config
                .map(|x| x.payload.get_json_field_as::<Ustr>("ip"))
                .unwrap_or_default(),
              gateway: config
                .map(|x| x.payload.get_json_field_as::<Ustr>("gateway"))
                .unwrap_or_default(),
              state: state.into(),
            });
          }
        }
      }
    }
    Message::from_type(MessageType::Ok).with_vec(statuses)
  } else if payload.unit_type == "netport" {
    Message::from_type(MessageType::Ok).with_vec(get_ports())
  } else {
    let mut units_map: HashMap<Ustr, UnitSerialized> = HashMap::new();

    if let Some(groups) = ctx.registry.metadata.groups("units") {
      for group in groups {
        let services = if let Some(services) = ctx
          .registry
          .metadata
          .group_items::<Service>("units", group.clone())
        {
          services
        } else {
          Vec::new()
        };
        let mounts = if let Some(mounts) = ctx
          .registry
          .metadata
          .group_items::<Mount>("units", group.clone())
        {
          mounts
        } else {
          Vec::new()
        };
        let sockets = if let Some(sockets) = ctx
          .registry
          .metadata
          .group_items::<Socket>("units", group.clone())
        {
          sockets
        } else {
          Vec::new()
        };
        let states = if let Some(states) = ctx
          .registry
          .metadata
          .group_items::<State>("units", group.clone())
        {
          states
        } else {
          Vec::new()
        };

        let signals = if let Some(signals) = ctx
          .registry
          .metadata
          .group_items::<Signal>("units", group.clone())
        {
          signals
        } else {
          Vec::new()
        };

        let mounted = mounts
          .iter()
          .filter(|mnt| is_mounted(&mnt.target).unwrap_or(false))
          .count();
        let active_services = services
          .iter()
          .filter(|s| {
            ctx
              .registry
              .instances::<Service>("units", Ustr::from(format!("{group}@{}", s.name)))
              .ok()
              .map_or(false, |x| x.iter().any(|x| x.instances.is_active()))
          })
          .count();
        let active_sockets = sockets
          .iter()
          .filter(|s| {
            ctx
              .registry
              .instances::<Socket>("units", Ustr::from(format!("{group}@{}", s.name)))
              .ok()
              .map_or(false, |x| x.iter().any(|x| x.active))
          })
          .count();
        let active_states = states
          .iter()
          .filter(|s| {
            sm.states
              .get(&Ustr::from(format!("{group}@{}", s.name)))
              .is_some()
          })
          .count();

        units_map.insert(
          group.clone(),
          UnitSerialized {
            active_services,
            active_sockets,
            active_states,
            mounted: mounted,
            mounts: mounts.len(),
            name: group.clone(),
            services: services.len(),
            sockets: sockets.len(),
            states: states.len(),
            signals: signals.len(),
          },
        );
      }
    }

    let mut units_list: Vec<UnitSerialized> = units_map.into_values().collect();
    units_list.sort_by(|a, b| a.name.cmp(&b.name));
    Message::from_type(MessageType::Ok).with(serialize_many(&units_list))
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
  build_ipc_list_response(payload, ctx)
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

pub fn handle_ipc_remove(
  _msg: Message,
  _ctx: &mut RuntimeContext<'_>,
  _dispatch: &RuntimeDispatcher,
  _log: &LogHandle,
) -> Result<Message, CoreError> {
  Ok(Message::default())
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

impl Runtime for IpcRuntime {
  fn id(&self) -> &str {
    IPC_RUNTIME_ID
  }

  fn handle(
    &mut self,
    action: &str,
    _payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      // "ipc:list" => {
      //   let msg = payload_msg(payload)?;
      //   return Ok(Some(handle_ipc_list(msg, ctx, dispatch, log)?.into()));
      // }
      "init_actions" => {
        let ipcsrc = ctx.scope.get::<IpcSourcemap>().cloned().unwrap_or_default();
        ipcsrc.register("login", handle_ipc_login, PERM_LOGIN);
        ipcsrc.register("logout", handle_ipc_logout, PermissionExpr::All);
        ipcsrc.register("run0", handle_ipc_run0, PermissionExpr::All);
        ipcsrc.register("start_service", handle_ipc_start, PermissionExpr::All);
        ipcsrc.register("stop_service", handle_ipc_stop, PermissionExpr::All);
        ipcsrc.register("start_socket", handle_ipc_start_socket, PermissionExpr::All);
        ipcsrc.register("stop_socket", handle_ipc_stop_socket, PermissionExpr::All);
        ipcsrc.register("list", handle_ipc_list, PermissionExpr::All);
        ipcsrc.register("network", handle_ipc_network, PERM_NETWORK);
        ipcsrc.register("set_variable", handle_ipc_set, PermissionExpr::All);
        ipcsrc.register("remove_variable", handle_ipc_remove, PermissionExpr::All);
        ipcsrc.register("reload_units", handle_ipc_reload_units, PermissionExpr::All);
        ipcsrc.register("reboot", handle_ipc_reboot, PermissionExpr::All);
        ipcsrc.register("soft_reboot", handle_ipc_soft_reboot, PermissionExpr::All);
        ipcsrc.register("shutdown", handle_ipc_shutdown, PermissionExpr::All);
      }
      "start_server" => {
        if self.listener_thread.is_none() {
          let tx = self.incoming_tx.clone();
          let notifier = ctx.notifier.clone();
          self.listener_thread = Some(thread::spawn(move || {
            let socket_path = "/tmp/rind.sock";
            let _ = std::fs::remove_file(socket_path);
            let listener = match UnixListener::bind(socket_path) {
              Ok(l) => l,
              Err(e) => {
                // TODO: i'll use log instead
                eprintln!("[ipc] failed to bind {}: {}", socket_path, e);
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
      "drain_requests" => {
        if let Ok(rx) = self.incoming_rx.lock() {
          while let Ok((msg, reply_tx)) = rx.try_recv() {
            let response = handle_ipc_message(msg, ctx, dispatch, log);
            let _ = reply_tx.send(response);
          }
        }
      }
      _ => {}
    }
    Ok(None)
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
    let mut len_buf = [0u8; 4];
    if stream.read_exact(&mut len_buf).is_err() {
      break;
    }
    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    if stream.read_exact(&mut buf).is_err() {
      break;
    }

    let raw = match String::from_utf8(buf) {
      Ok(s) => s,
      Err(_) => continue,
    };

    let msg: Message = match serde_json::from_str::<Message>(&raw) {
      Ok(m) => {
        if cred.uid == 0 && m.from_uid.is_some() {
          m
        } else {
          m.from_gid(cred.gid).from_uid(cred.uid).from_pid(cred.pid)
        }
      }
      Err(_) => continue,
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

    let resp_str = response.as_string().into_bytes();
    let resp_len = (resp_str.len() as u32).to_be_bytes();

    if stream.write_all(&resp_len).is_err() {
      break;
    }
    if stream.write_all(&resp_str).is_err() {
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
      .with(format!("Message handler not found: {:?}", msg.action));
  };

  drop(ipcsrc_shared);

  if !matches!(&source.perms, PermissionExpr::All)
    && !pm.user_check(msg.from_uid.unwrap_or(0), &source.perms)
  {
    return Message::from_type(MessageType::Error).with(format!("Permission Denied"));
  }

  let mut fields = HashMap::new();
  fields.insert("name".to_string(), msg.action.clone());
  log.log(LogLevel::Trace, "ipc-runtime", "ipc call", fields);

  match (source.handler)(msg, ctx, dispatch, log) {
    Ok(resp) => resp,
    Err(e) => Message::from_type(MessageType::Error).with(format!("IPC handler failed: {e}")),
  }
}
