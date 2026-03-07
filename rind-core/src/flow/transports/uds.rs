use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use crate::services::Service;
use rind_common::error::{report_error, rw_write};

use super::*;

type ClientMap = Arc<Mutex<HashMap<String, Vec<UnixStream>>>>;

fn socket_path(endpoint: &str) -> std::path::PathBuf {
  std::path::PathBuf::from("/run/rind-tp").join(format!("{endpoint}.sock"))
}

fn start_listener(endpoint: String, path: std::path::PathBuf, clients: ClientMap) {
  if let Some(parent) = path.parent() {
    let _ = std::fs::create_dir_all(parent);
  }
  let _ = std::fs::remove_file(&path);
  let listener = match UnixListener::bind(&path) {
    Ok(listener) => listener,
    Err(err) => {
      report_error(
        "uds transport bind failed",
        format!("{}: {err}", path.display()),
      );
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
        locked
          .entry(endpoint.clone())
          .or_insert_with(Vec::new)
          .push(writer);
      }

      let endpoint_for_msg = endpoint.clone();
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
            .handle_message(endpoint_for_msg.clone(), msg);
        }
      });
    }
  });
}

#[derive(Default)]
pub struct UdsTransportProtocol {
  clients: ClientMap,
  started_endpoints: std::collections::HashSet<String>,
}

impl TransportProtocol for UdsTransportProtocol {
  fn init(
    &mut self,
    _options: Vec<String>,
    ctx: &TransportInitContext,
    _service: Option<&mut Service>,
  ) {
    let endpoint = ctx.endpoint.to_string();
    if self.started_endpoints.contains(&endpoint) {
      return;
    }
    start_listener(
      endpoint.clone(),
      socket_path(ctx.endpoint),
      self.clients.clone(),
    );
    self.started_endpoints.insert(endpoint);
  }

  fn recv(
    &self,
    ctx: &mut TransportContext,
    instance: &FlowInstance,
    service: Option<&mut Service>,
  ) -> TransportResult {
    let endpoint = if let Some(service) = service {
      format!("{}@{}", service.unit.to_string(), service.name)
    } else {
      instance.name.clone()
    };

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
    if let Ok(mut locked) = self.clients.lock()
      && let Some(clients) = locked.get_mut(&endpoint)
    {
      clients.retain_mut(|stream| stream.write_all(frame.as_bytes()).is_ok());
    }

    Ok(None)
  }
}
