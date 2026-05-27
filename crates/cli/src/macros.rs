#[macro_export]
macro_rules! handle_send {
  ($action:expr,$payload:expr) => {
    handle_send_raw!($action, ser_to_vec($payload, false));
  };
}

#[macro_export]
macro_rules! send_msg {
  ($action:expr,$payload:expr) => {
    send_message(Message::from($action).with($payload.into()))
  };
}

#[macro_export]
macro_rules! handle_send_raw {
  ($action:expr,$payload:expr) => {
    handle_message(match send_msg!($action, $payload) {
      Ok(e) => e,
      Err(e) => Message::from_type(MessageType::Error).with_string(format!("{e}")),
    });
  };
}
