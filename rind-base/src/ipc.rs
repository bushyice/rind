use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::mount::{Mount, is_mounted};
use crate::services::Service;
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
                // TODO: i'll use log instead
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

    let msg: Message = match serde_json::from_str(&raw) {
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
      let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
        return Message::from_type(MessageType::Error)
          .with(format!("Incorrect Payload: {:?}", msg.payload));
      };

      if payload.unit_type == UnitType::Unit {
        let services = if let Some(services) = ctx
          .registry
          .metadata
          .group_items::<Service>("units", &payload.name)
        {
          services
        } else {
          Vec::new()
        };
        let mounts = if let Some(mounts) = ctx
          .registry
          .metadata
          .group_items::<Mount>("units", &payload.name)
        {
          mounts
        } else {
          Vec::new()
        };

        let ser_instances: HashMap<&String, (String, Vec<u32>)> = services
          .iter()
          .filter_map(|ser| {
            ctx
              .registry
              .as_one::<Service>("units", &format!("{}@{}", payload.name, ser.name))
              .map_or(None, |x| {
                Some((&ser.name, (x.instances.last_state(), x.instances.pid())))
              })
          })
          .collect();

        Message::from_type(MessageType::List).with(
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
                run: svc.run.to_string(),
                last_state: ser_instances
                  .get(&svc.name)
                  .map_or("Inactive".into(), |x| x.0.clone()),
                name: svc.name().to_string(),
                pid: ser_instances.get(&svc.name).map_or(None, |x| Some(x.1[0])),
                restart: svc.restart.as_ref().map_or(false, |_| true),
              })
              .collect(),
          }
          .stringify(),
        )
      } else if payload.unit_type == UnitType::Service {
        let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
          return Message::from_type(MessageType::Error)
            .with(format!("Incorrect Payload: {:?}", msg.payload));
        };

        let Some(service_meta) = ctx
          .registry
          .metadata
          .find::<Service>("units", &payload.name)
        else {
          return Message::from_type(MessageType::Error)
            .with(format!("Service not found: {}", payload.name));
        };

        let service = if let Ok(s) = ctx.registry.as_one::<Service>("units", &payload.name) {
          s
        } else {
          &Service::new(service_meta)
        };

        Message::from_type(MessageType::List).with(
          ServiceSerialized {
            name: service.metadata.name.clone(),
            after: service.metadata.after.clone(),
            last_state: service.instances.last_state(),
            pid: service.instances.pid().get(0).cloned(),
            restart: service.metadata.restart.as_ref().map_or(false, |_| true),
            run: service.metadata.run.to_string(),
          }
          .stringify(),
        )
      } else {
        let mut units_map: HashMap<String, UnitSerialized> = HashMap::new();

        if let Some(groups) = ctx.registry.metadata.groups("units") {
          for group in groups {
            let services = if let Some(services) = ctx
              .registry
              .metadata
              .group_items::<Service>("units", &group)
            {
              services
            } else {
              Vec::new()
            };
            let mounts =
              if let Some(mounts) = ctx.registry.metadata.group_items::<Mount>("units", &group) {
                mounts
              } else {
                Vec::new()
              };

            let mounted = mounts
              .iter()
              .filter(|mnt| is_mounted(&mnt.target).unwrap_or(false))
              .count();
            let active = services
              .iter()
              .filter(|s| {
                ctx
                  .registry
                  .instances::<Service>("units", &format!("{group}@{}", s.name))
                  .ok()
                  .map_or(false, |x| x.iter().any(|x| x.instances.is_active()))
              })
              .count();
            units_map.insert(
              group.to_string(),
              UnitSerialized {
                active_services: active,
                mounted: mounted,
                mounts: mounts.len(),
                name: group.to_string(),
                services: services.len(),
              },
            );
          }
        }

        let mut units_list: Vec<UnitSerialized> = units_map.into_values().collect();
        units_list.sort_by(|a, b| a.name.cmp(&b.name));
        Message::from_type(MessageType::List).with(serialize_many(&units_list))
      }
    }
    MessageType::Start => {
      let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
        return Message::nack("`start payload");
      };

      let _ = dispatch.dispatch(
        "services",
        "start",
        serde_json::json!({ "name": payload.name }).into(),
      );
      Message::ack(format!("started {}", payload.name))
    }
    MessageType::Stop => {
      let Some(payload) = msg.parse_payload::<MessagePayload>().ok() else {
        return Message::nack("invalid stop payload");
      };

      let force = payload.force.unwrap_or(false);
      let _ = dispatch.dispatch("services", "stop", serde_json::json!({ "name": payload.name, "mode": if force { "force" } else { "graceful" } }).into());
      Message::ack(format!("stopped {}", payload.name))
    }
    _ => Message::from_type(MessageType::Unknown),
  }
}
