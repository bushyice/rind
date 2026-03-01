use rind_ipc::{Message, MessageType, recv::start_ipc_server, ser::UnitsSerialized};

fn handle_client(msg: Message) -> Result<Option<Message>, anyhow::Error> {
  let units_ser = UnitsSerialized::from_registry();
  // let units = { UNITS.read().unwrap() };
  Ok(Some(match msg.r#type {
    MessageType::List => Message::from_type(MessageType::List).with(units_ser.to_string()),
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
