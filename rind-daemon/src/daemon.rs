use rind_common::error::install_panic_handler;
use rind_core::{
  error::{rw_read, rw_write},
  flow::StateDefinition,
  loginfo, logtrc, logwarn,
  mount::Mount,
  services::{RestartPolicy, StopMode, start_service, stop_service},
  store::{PersistMode, STORE},
};
use rind_ipc::{
  Message, MessagePayload, MessageType, Service, ServiceState, UnitType,
  recv::start_ipc_server,
  ser::{
    MountSerialized, ServiceSerialized, StateSerialized, UnitItemsSerialized, UnitSerialized,
    serialize_many,
  },
};

fn handle_client(msg: Message) -> Result<Option<Message>, anyhow::Error> {
  Ok(Some(match msg.r#type {
    MessageType::List => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Ok(Some(
          Message::from_type(MessageType::Error).with(format!("Payload Incorrect")),
        ));
      };
      let store = { rw_read(&STORE, "store read in daemon list") };

      if matches!(payload.unit_type, UnitType::Unit) {
        if let Some(unit) = store.unit(payload.name) {
          let services = if let Some(services) = &unit.service {
            services
              .iter()
              .map(|x| ServiceSerialized {
                name: x.name.clone(),
                last_state: format!("{:?}", x.state),
                after: x.after.clone(),
                restart: if matches!(x.restart, RestartPolicy::Bool(false)) {
                  false
                } else {
                  true
                },
                args: x.args.clone(),
                exec: x.exec.clone(),
                pid: x.child.as_ref().map(|x| x.id()),
              })
              .collect::<Vec<ServiceSerialized>>()
          } else {
            Vec::new()
          };
          let mounts = if let Some(mounts) = &unit.mount {
            mounts
              .iter()
              .map(|x| MountSerialized {
                source: x.source.clone(),
                target: x.target.clone(),
                fstype: x.fstype.clone(),
                mounted: x.is_mounted(),
              })
              .collect::<Vec<MountSerialized>>()
          } else {
            Vec::new()
          };
          Message::from_type(MessageType::List)
            .with(UnitItemsSerialized { mounts, services }.stringify())
        } else {
          Message::from_type(MessageType::Error).with(format!("Unit not found"))
        }
      } else if matches!(payload.unit_type, UnitType::Service) {
        if let Some(x) = store.lookup::<Service>(&payload.name) {
          Message::from_type(MessageType::List).with(
            ServiceSerialized {
              name: x.name.clone(),
              last_state: format!("{:?}", x.state),
              after: x.after.clone(),
              restart: if matches!(x.restart, RestartPolicy::Bool(false)) {
                false
              } else {
                true
              },
              args: x.args.clone(),
              exec: x.exec.clone(),
              pid: x.child.as_ref().map(|x| x.id()),
            }
            .stringify(),
          )
        } else {
          Message::from_type(MessageType::Error).with(format!("Service not found"))
        }
      } else if matches!(payload.unit_type, UnitType::State) {
        if let Some(x) = store.lookup::<StateDefinition>(&payload.name) {
          Message::from_type(MessageType::List).with(
            StateSerialized {
              name: x.name.clone(),
              instances: store.state_branches(&x.name).unwrap_or(&Vec::new()).len(),
            }
            .stringify(),
          )
        } else {
          Message::from_type(MessageType::Error).with(format!("Service not found"))
        }
      } else {
        Message::from_type(MessageType::List).with(serialize_many(
          &store
            .names()
            .filter_map(|name| {
              let Some(unit) = store.unit(name) else {
                return None;
              };

              UnitSerialized {
                name: name.to_string(),
                services: unit.len::<Service>(),
                active_services: unit
                  .len_for::<Service>(|x| matches!(x.state, ServiceState::Active)),
                mounts: unit.len::<Mount>(),
                mounted: unit.len_for::<Mount>(|x| x.is_mounted()),
              }
              .as_some()
            })
            .collect::<Vec<UnitSerialized>>(),
        ))
      }
    }
    MessageType::Start => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Ok(Some(Message::nack("invalid start payload")));
      };

      logtrc!("Start request for: {:?}", payload.name);

      let mut units = rw_write(&STORE, "store write in daemon start");

      if let Some(ser) = units.lookup_mut::<Service>(&payload.name) {
        start_service(ser);
      } else {
        let err = format!("Unit component not found: {:?}", payload.name);
        loginfo!("{err}");
        return Ok(Some(Message::nack(err)));
      }

      Message::ack(format!("started {}", payload.name))
    }
    MessageType::Stop => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Ok(Some(Message::nack("invalid stop payload")));
      };
      let force = payload.force.unwrap_or(false);
      let stop_mode = if force {
        StopMode::ForceKill
      } else {
        StopMode::Graceful
      };

      logtrc!("Stop request for: {:?}", payload.name);

      if force {
        logwarn!("Force stopping {:?}", payload.name);
      }

      let mut units = rw_write(&STORE, "store write in daemon stop");

      if let Some(ser) = units.lookup_mut::<Service>(&payload.name) {
        stop_service(ser, stop_mode);
      } else {
        let err = format!("Unit component not found: {:?}", payload.name);
        loginfo!("{err}");
        return Ok(Some(Message::nack(err)));
      }

      Message::ack(format!("stopped {}", payload.name))
    }
    MessageType::Enable => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Ok(Some(Message::nack("invalid enable payload")));
      };
      let target_name = payload.name.clone();

      let mut units = rw_write(&STORE, "store write in daemon enable");

      if let Some((unit_name, thing)) = payload.name.split_once('@') {
        if thing == "*" {
          units.enable_unit(unit_name, PersistMode::Yes);
        }
        units.enable_component(unit_name, thing, PersistMode::Yes);
      } else {
        units.enable_unit(payload.name, PersistMode::Yes);
      }

      Message::ack(format!("enabled {}", target_name))
    }
    MessageType::Disable => {
      let Some(payload) = msg.parse_payload::<MessagePayload>() else {
        return Ok(Some(Message::nack("invalid disable payload")));
      };
      let target_name = payload.name.clone();

      let mut units = rw_write(&STORE, "store write in daemon disable");

      if let Some((unit_name, thing)) = payload.name.split_once('@') {
        if thing == "*" {
          units.disable_unit(unit_name, PersistMode::Yes);
        }
        units.disable_component(unit_name, thing, PersistMode::Yes);
      } else {
        units.disable_unit(payload.name, PersistMode::Yes);
      }

      Message::ack(format!("disabled {}", target_name))
    }
    _ => MessageType::Unknown.into(),
  }))
}

pub fn start_daemon() -> anyhow::Result<()> {
  install_panic_handler("daemon");
  start_ipc_server(handle_client)?;
  Ok(())
}
