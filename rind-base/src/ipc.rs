use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::mount::Mount;
use crate::services::{Service, ServiceState};
use rind_core::prelude::*;
use rind_ipc::{
  Message, MessagePayload, MessageType, UnitType,
  ser::{MountSerialized, ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many},
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
  ) -> Result<(), CoreError> {
    match action {
      "start_server" => {
        if self.listener_thread.is_none() {
          let tx = self.incoming_tx.clone();
          self.listener_thread = Some(thread::spawn(move || {
            let socket_path = "/tmp/rind.sock";
            let _ = std::fs::remove_file(socket_path);
            let listener = match UnixListener::bind(socket_path) {
              Ok(l) => l,
              Err(e) => {
                eprintln!("[ipc] failed to bind {}: {}", socket_path, e);
                return;
              }
            };

            for stream in listener.incoming() {
              if let Ok(stream) = stream {
                let tx = tx.clone();
                thread::spawn(move || {
                  handle_client_connection(stream, tx);
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
    Ok(())
  }
}

fn handle_client_connection(mut stream: UnixStream, parent_tx: Sender<IpcRequest>) {
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

    let msg: Message = match toml::from_str(&raw) {
      Ok(m) => m,
      Err(_) => continue,
    };

    let (reply_tx, reply_rx) = mpsc::channel::<Message>();
    if parent_tx.send((msg, reply_tx)).is_err() {
      break;
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
  _log: &LogHandle,
) -> Message {
  match msg.r#type {
    MessageType::List => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Message::from_type(MessageType::Error).with("Payload Incorrect".into());
      };

      if matches!(payload.unit_type, UnitType::Unit) {
        let mut services = Vec::new();
        let mut mounts = Vec::new();

        if let Some(instances) = ctx.registry.instances.get("units") {
          for inst in instances {
            if let Some(service) = inst.downcast_ref::<Service>() {
              if service.metadata.name.starts_with(&payload.name) {
                let state_str = format!(
                  "{:?}",
                  service
                    .instances
                    .0
                    .first()
                    .map(|i| &i.state)
                    .unwrap_or(&ServiceState::Inactive)
                );
                let pid = service
                  .instances
                  .0
                  .first()
                  .and_then(|i| i.child.as_ref().map(|c| c.id()));

                services.push(ServiceSerialized {
                  name: service.metadata.name.clone(),
                  last_state: state_str,
                  after: service.metadata.after.clone(),
                  restart: service.metadata.restart.is_some(),
                  args: service
                    .metadata
                    .run
                    .as_many()
                    .next()
                    .map(|r| r.args.clone())
                    .unwrap_or_default(),
                  exec: service
                    .metadata
                    .run
                    .as_many()
                    .next()
                    .map(|r| r.exec.clone())
                    .unwrap_or_default(),
                  pid,
                });
              }
            } else if let Some(mount) = inst.downcast_ref::<Mount>() {
              if mount.metadata.name().starts_with(&payload.name) {
                mounts.push(MountSerialized {
                  source: mount.metadata.source.clone(),
                  target: mount.metadata.target.clone(),
                  fstype: mount.metadata.fstype.clone(),
                  mounted: mount.is_mounted,
                });
              }
            }
          }
        }

        Message::from_type(MessageType::List)
          .with(UnitItemsSerialized { mounts, services }.stringify())
      } else if matches!(payload.unit_type, UnitType::Service) {
        if let Some(instances) = ctx.registry.instances.get("units") {
          if let Some(inst) = instances.iter().find(|i| {
            i.downcast_ref::<Service>()
              .map(|s| s.metadata.name == payload.name)
              .unwrap_or(false)
          }) {
            let service = inst.downcast_ref::<Service>().unwrap();
            let state_str = format!(
              "{:?}",
              service
                .instances
                .0
                .first()
                .map(|i| &i.state)
                .unwrap_or(&ServiceState::Inactive)
            );
            let pid = service
              .instances
              .0
              .first()
              .and_then(|i| i.child.as_ref().map(|c| c.id()));

            return Message::from_type(MessageType::List).with(
              ServiceSerialized {
                name: service.metadata.name.clone(),
                last_state: state_str,
                after: service.metadata.after.clone(),
                restart: service.metadata.restart.is_some(),
                args: service
                  .metadata
                  .run
                  .as_many()
                  .next()
                  .map(|r| r.args.clone())
                  .unwrap_or_default(),
                exec: service
                  .metadata
                  .run
                  .as_many()
                  .next()
                  .map(|r| r.exec.clone())
                  .unwrap_or_default(),
                pid,
              }
              .stringify(),
            );
          }
        }
        Message::from_type(MessageType::Error).with("Service not found".into())
      } else {
        let mut units_map: HashMap<String, UnitSerialized> = HashMap::new();

        if let Some(instances) = ctx.registry.instances.get("units") {
          for inst in instances {
            if let Some(service) = inst.downcast_ref::<Service>() {
              let unit_name = service
                .metadata
                .name
                .split('@')
                .next()
                .unwrap_or(&service.metadata.name)
                .to_string();
              let entry = units_map
                .entry(unit_name.clone())
                .or_insert(UnitSerialized {
                  name: unit_name,
                  services: 0,
                  active_services: 0,
                  mounts: 0,
                  mounted: 0,
                });

              entry.services += 1;
              let is_active = service
                .instances
                .0
                .first()
                .map(|i| matches!(i.state, ServiceState::Active | ServiceState::Starting))
                .unwrap_or(false);
              if is_active {
                entry.active_services += 1;
              }
            } else if let Some(mount) = inst.downcast_ref::<Mount>() {
              let unit_name = mount
                .metadata
                .name()
                .split('@')
                .next()
                .unwrap_or(&mount.metadata.name())
                .to_string();
              let entry = units_map
                .entry(unit_name.clone())
                .or_insert(UnitSerialized {
                  name: unit_name,
                  services: 0,
                  active_services: 0,
                  mounts: 0,
                  mounted: 0,
                });

              entry.mounts += 1;
              if mount.is_mounted {
                entry.mounted += 1;
              }
            }
          }
        }

        let mut units_list: Vec<UnitSerialized> = units_map.into_values().collect();
        units_list.sort_by(|a, b| a.name.cmp(&b.name));
        Message::from_type(MessageType::List).with(serialize_many(&units_list))
      }
    }
    MessageType::Start => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Message::nack("invalid start payload");
      };

      let _ = dispatch.dispatch(
        "services",
        "start",
        serde_json::json!({ "name": payload.name }).into(),
      );
      Message::ack(format!("started {}", payload.name))
    }
    MessageType::Stop => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Message::nack("invalid stop payload");
      };

      let force = payload.force.unwrap_or(false);
      let _ = dispatch.dispatch("services", "stop", serde_json::json!({ "name": payload.name, "mode": if force { "force" } else { "graceful" } }).into());
      Message::ack(format!("stopped {}", payload.name))
    }
    _ => Message::from_type(MessageType::Unknown),
  }
}
