#[macro_export]
macro_rules! handle {
  ($message:expr) => {
    handle_message(match send_message($message) {
      Ok(e) => e,
      Err(e) => Message::from_type(MessageType::Error).with(format!("{e}")),
    });
  };
}
