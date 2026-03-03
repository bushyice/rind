use rind_core::{
  loginfo, logtrc, logwarn,
  mount::Mount,
  services::{RestartPolicy, start_service, stop_service},
  sockets::Socket,
  store::STORE,
};
use rind_ipc::{
  Message, MessageType, Payload, Service, ServiceState, UnitType,
  recv::start_ipc_server,
  ser::{MountSerialized, ServiceSerialized, UnitItemsSerialized, UnitSerialized, serialize_many},
};

fn handle_client(msg: Message) -> Result<Option<Message>, anyhow::Error> {
  Ok(Some(match msg.r#type {
    MessageType::List => {
      let Some(payload) = msg.parse_payload::<Payload>() else {
        return Ok(Some(
          Message::from_type(MessageType::Error).with(format!("Payload Incorrect")),
        ));
      };
      let store = { STORE.read().unwrap() };

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
      let Some(payload) = msg.parse_payload::<Payload>() else {
        return Ok(Some(MessageType::Unknown.into()));
      };

      logtrc!("Start request for: {:?}", payload.name);

      let mut units = STORE.write().unwrap();

      if let Some(ser) = units.lookup_mut::<Service>(&payload.name) {
        start_service(ser);
      } else if let Some(_soc) = units.lookup_mut::<Socket>(&payload.name) {
      } else {
        let err = format!("Unit component not found: {:?}", payload.name);
        loginfo!("{err}");
        return Ok(Some(Message::from_type(MessageType::Error).with(err)));
      }

      MessageType::Unknown.into()
    }
    MessageType::Stop => {
      let Some(payload) = msg.parse_payload::<Payload>() else {
        return Ok(Some(MessageType::Unknown.into()));
      };
      let force = payload.force.unwrap_or(false);

      logtrc!("Stop request for: {:?}", payload.name);

      if force {
        logwarn!("Force stopping {:?}", payload.name);
      }

      let mut units = STORE.write().unwrap();

      if let Some(ser) = units.lookup_mut::<Service>(&payload.name) {
        stop_service(ser, force);
      } else if let Some(_soc) = units.lookup_mut::<Socket>(&payload.name) {
      } else {
        let err = format!("Unit component not found: {:?}", payload.name);
        loginfo!("{err}");
        return Ok(Some(Message::from_type(MessageType::Error).with(err)));
      }

      MessageType::Unknown.into()
    }
    MessageType::Enable => {
      let Some(payload) = msg.parse_payload::<Payload>() else {
        return Ok(Some(MessageType::Unknown.into()));
      };

      let mut units = STORE.write().unwrap();

      if let Some((unit_name, thing)) = payload.name.split_once('@') {
        if thing == "*" {
          units.enable_unit(unit_name, true);
        }
        units.enable_component(unit_name, thing, true);
      } else {
        units.enable_unit(payload.name, true);
      }

      MessageType::Unknown.into()
    }
    MessageType::Disable => {
      let Some(payload) = msg.parse_payload::<Payload>() else {
        return Ok(Some(MessageType::Unknown.into()));
      };

      let mut units = STORE.write().unwrap();

      if let Some((unit_name, thing)) = payload.name.split_once('@') {
        if thing == "*" {
          units.disable_unit(unit_name, true);
        }
        units.disable_component(unit_name, thing, true);
      } else {
        units.disable_unit(payload.name, true);
      }

      MessageType::Unknown.into()
    }
    _ => MessageType::Unknown.into(),
  }))
}

pub fn start_daemon() -> anyhow::Result<()> {
  start_ipc_server(handle_client)?;
  Ok(())
}
