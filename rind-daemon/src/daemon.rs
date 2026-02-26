use rind_ipc::{Message, MessageType, recv::start_ipc_server, ser::UnitsSerialized};

fn handle_client(msg: Message) -> Result<Option<Message>, anyhow::Error> {
  let units_ser = UnitsSerialized::from_registry();
  // let units = { UNITS.read().unwrap() };
  Ok(Some(match msg.r#type {
    MessageType::List => Message::from_type(MessageType::List).with(units_ser.to_string()),
    _ => MessageType::Unknown.into(),
  }))
}

pub fn start_daemon() -> anyhow::Result<()> {
  start_ipc_server(handle_client)?;
  Ok(())
}
