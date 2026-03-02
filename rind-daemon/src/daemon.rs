use rind_core::{
  loginfo, logtrc, logwarn,
  services::{start_service, stop_service},
  sockets::Socket,
  units::UNITS,
};
use rind_ipc::{
  Message, MessageType, Payload, Service, recv::start_ipc_server, ser::UnitsSerialized,
};

fn handle_client(msg: Message) -> Result<Option<Message>, anyhow::Error> {
  let units_ser = UnitsSerialized::from_registry();
  // let units = { UNITS.read().unwrap() };
  Ok(Some(match msg.r#type {
    MessageType::List => Message::from_type(MessageType::List).with(units_ser.to_string()),
    MessageType::Start => {
      let Some(payload) = msg.parse_payload::<Payload>() else {
        return Ok(Some(MessageType::Unknown.into()));
      };

      logtrc!("Start request for: {:?}", payload.name);

      let mut units = UNITS.write().unwrap();

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

      let mut units = UNITS.write().unwrap();

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
    _ => MessageType::Unknown.into(),
  }))
}

const SKIP: [&'static str; 3] = ["/proc", "/sys", "/dev"];

fn visit_dirs(path: &std::path::Path) {
  if let Ok(entries) = std::fs::read_dir(path) {
    for entry in entries.flatten() {
      let path = entry.path();
      println!("{}", path.display());

      if SKIP.contains(&path.to_str().unwrap()) {
        continue;
      }

      if path.is_dir() {
        visit_dirs(&path);
      }
    }
  }
}

pub fn start_daemon() -> anyhow::Result<()> {
  visit_dirs(std::path::Path::new("/usr/bin"));
  start_ipc_server(handle_client)?;
  Ok(())
}
