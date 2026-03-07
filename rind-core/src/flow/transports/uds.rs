use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use crate::services::Service;
use rind_common::error::{report_error, rw_write};

use super::*;

fn socket_path() -> String {
  std::env::var("RIND_FLOW_UDS_PATH").unwrap_or_else(|_| "/tmp/rind-flow.sock".to_string())
}

fn start_listener(path: String, clients: Arc<Mutex<Vec<UnixStream>>>) {
  let _ = std::fs::remove_file(&path);
  let listener = match UnixListener::bind(&path) {
    Ok(listener) => listener,
    Err(err) => {
      report_error("uds transport bind failed", format!("{path}: {err}"));
      return;
    }
  };

  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(stream) = stream else {
        continue;
      };

      if let Ok(writer) = stream.try_clone()
        && let Ok(mut locked) = clients.lock()
      {
        locked.push(writer);
      }

      std::thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        loop {
          line.clear();
          let Ok(read) = reader.read_line(&mut line) else {
            break;
          };
          if read == 0 {
            break;
          }

          let payload = line.trim();
          if payload.is_empty() {
            continue;
          }

          let Ok(msg) = serde_json::from_str::<TransportMessage>(payload) else {
            report_error("uds transport parse error", payload);
            continue;
          };

          rw_write(&crate::store::STORE, "store write in uds listener")
            .handle_message("uds".to_string(), msg);
        }
      });
    }
  });
}

pub struct UdsTransportProtocol {
  clients: Arc<Mutex<Vec<UnixStream>>>,
}

impl Default for UdsTransportProtocol {
  fn default() -> Self {
    let clients = Arc::new(Mutex::new(Vec::new()));
    start_listener(socket_path(), clients.clone());
    Self { clients }
  }
}

impl TransportProtocol for UdsTransportProtocol {
  fn recv(
    &self,
    ctx: &mut TransportContext,
    instance: &FlowInstance,
    _service: Option<&mut Service>,
  ) -> TransportResult {
    let msg = serde_json::to_string(&TransportMessage {
      action: ctx.action,
      name: Some(instance.name.clone()),
      payload: Some(instance.payload.clone()),
      r#type: if instance.r#type == FlowType::Signal {
        TransportMessageType::Signal
      } else {
        TransportMessageType::State
      },
    })?;

    let frame = format!("{msg}\n");
    if let Ok(mut clients) = self.clients.lock() {
      clients.retain_mut(|stream| stream.write_all(frame.as_bytes()).is_ok());
    }

    Ok(None)
  }

  fn init(&mut self, _options: Vec<String>, _service: Option<&mut crate::services::Service>) {}
}
