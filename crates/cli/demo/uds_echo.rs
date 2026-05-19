use rind_core::prelude::*;
use rind_ipc::{FlowPayload, TransportMessage, TransportMessageAction, TransportMessageType};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

fn endpoint() -> String {
  std::env::var("RIND_UDS_ENDPOINT").unwrap_or_else(|_| "tp_demo:uds_echo".to_string())
}

fn socket_path() -> String {
  format!("/run/rind-tp/{}.sock", endpoint())
}

fn connect_with_retry(path: &str) -> UnixStream {
  loop {
    match UnixStream::connect(path) {
      Ok(stream) => return stream,
      Err(err) => {
        eprintln!("waiting for {path}: {err}");
        thread::sleep(Duration::from_millis(300));
      }
    }
  }
}

fn main() {
  let path = socket_path();
  let mut stream = connect_with_retry(path.as_str());

  println!("example-uds connected to {path}");
  loop {
    let msg = match TransportMessage::read_signed(&mut stream) {
      Ok(m) => m,
      Err(_) => break,
    };

    println!("uds_in: {:?}", msg.name);

    let payload_str = if let Some(FlowPayload::String(s)) = msg.payload {
      s
    } else {
      "unknown".to_string()
    };

    let reply = TransportMessage {
      r#type: TransportMessageType::Impulse,
      name: Some("tp_demo:demo_ping".to_ustr()),
      payload: Some(FlowPayload::String(format!("echo:{}", payload_str))),
      action: TransportMessageAction::Set,
      branch: None,
    };

    if reply.write_signed(&mut stream).is_err() {
      break;
    }
  }
}
