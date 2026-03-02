#[macro_export]
macro_rules! handle {
  ($message:expr) => {
    handle_message(match send_message($message) {
      Ok(e) => e,
      Err(e) => Message::from_type(MessageType::Error).with(format!("{e}")),
    });
  };
}

#[macro_export]
macro_rules! action {
  ($type:ident,$name:expr,$unit:ident,$force:expr) => {
    Message::from_type(MessageType::$type).with_payload(Payload {
      force: $force,
      name: $name,
      unit_type: rind_ipc::UnitType::$unit,
    })
  };
}
