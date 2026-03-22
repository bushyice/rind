use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use rind_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::flow::{FlowMatchOperation, FlowPayload};

#[derive(Serialize, Deserialize, Copy, Clone)]
pub enum TransportMessageType {
  Signal,
  State,
  Enquiry,
  Response,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Copy, Clone)]
pub enum TransportMessageAction {
  #[default]
  Set,
  Remove,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TransportMessage {
  pub r#type: TransportMessageType,
  pub payload: Option<FlowPayload>,
  pub branch: Option<FlowMatchOperation>,
  pub name: Option<String>,
  #[serde(default)]
  pub action: TransportMessageAction,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct TransportProtocolId(pub String);

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum TransportMethod {
  Type(TransportProtocolId),
  Options {
    id: TransportProtocolId,
    options: Vec<String>,
  },
  Object {
    id: TransportProtocolId,
    options: serde_json::Value,
  },
}

pub trait TransportProtocol: Send + Sync {
  fn setup(&mut self, endpoint: &str);
  fn send_message(&self, endpoint: &str, msg: &TransportMessage);
}

type ClientMap = Arc<Mutex<HashMap<String, Vec<UnixStream>>>>;

fn socket_path(endpoint: &str) -> std::path::PathBuf {
  std::path::PathBuf::from("/run/rind-tp").join(format!("{endpoint}.sock"))
}

pub struct UdsTransport {
  clients: ClientMap,
  started: std::collections::HashSet<String>,
  incoming_tx: std::sync::mpsc::Sender<(String, TransportMessage)>,
  incoming_rx: Arc<Mutex<std::sync::mpsc::Receiver<(String, TransportMessage)>>>,
}

impl Default for UdsTransport {
  fn default() -> Self {
    let (tx, rx) = std::sync::mpsc::channel();
    Self {
      clients: Arc::new(Mutex::new(HashMap::new())),
      started: std::collections::HashSet::new(),
      incoming_tx: tx,
      incoming_rx: Arc::new(Mutex::new(rx)),
    }
  }
}

impl UdsTransport {
  fn start_listener(&self, endpoint: String) {
    let path = socket_path(&endpoint);
    if let Some(parent) = path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
      Ok(l) => l,
      Err(e) => {
        eprintln!("[transport] uds bind failed {}: {e}", path.display());
        return;
      }
    };

    let clients = self.clients.clone();
    let tx = self.incoming_tx.clone();
    let ep = endpoint.clone();

    std::thread::spawn(move || {
      for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };

        if let Ok(writer) = stream.try_clone() {
          if let Ok(mut locked) = clients.lock() {
            locked.entry(ep.clone()).or_default().push(writer);
          }
        }

        let tx = tx.clone();
        let ep_for_msg = ep.clone();
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
            let trimmed = line.trim();
            if trimmed.is_empty() {
              continue;
            }
            if let Ok(msg) = serde_json::from_str::<TransportMessage>(trimmed) {
              let _ = tx.send((ep_for_msg.clone(), msg));
            }
          }
        });
      }
    });
  }
}

impl TransportProtocol for UdsTransport {
  fn setup(&mut self, endpoint: &str) {
    if self.started.contains(endpoint) {
      return;
    }
    self.start_listener(endpoint.to_string());
    self.started.insert(endpoint.to_string());
  }

  fn send_message(&self, endpoint: &str, msg: &TransportMessage) {
    let frame = match serde_json::to_string(msg) {
      Ok(s) => format!("{s}\n"),
      Err(_) => return,
    };
    if let Ok(mut locked) = self.clients.lock() {
      if let Some(streams) = locked.get_mut(endpoint) {
        streams.retain_mut(|stream| stream.write_all(frame.as_bytes()).is_ok());
      }
    }
  }
}

pub fn start_stdout_listener(
  service_name: String,
  child: &mut std::process::Child,
  tx: std::sync::mpsc::Sender<(String, TransportMessage)>,
) {
  if let Some(stdout) = child.stdout.take() {
    std::thread::spawn(move || {
      let reader = BufReader::new(stdout);
      for line in reader.lines().flatten() {
        if let Ok(msg) = serde_json::from_str::<TransportMessage>(&line) {
          let _ = tx.send((service_name.clone(), msg));
        }
      }
    });
  }
}

pub struct TransportRuntime {
  uds: UdsTransport,
  stdio_endpoints: std::collections::HashSet<String>,
}

impl Default for TransportRuntime {
  fn default() -> Self {
    Self {
      uds: UdsTransport::default(),
      stdio_endpoints: std::collections::HashSet::new(),
    }
  }
}

impl Runtime for TransportRuntime {
  fn id(&self) -> &str {
    "transport"
  }

  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    _ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<(), CoreError> {
    match action {
      "setup_uds" => {
        let endpoint = payload.get::<String>("endpoint")?;
        self.uds.setup(&endpoint);
      }
      "register_stdio" => {
        let endpoint = payload.get::<String>("endpoint")?;
        self.stdio_endpoints.insert(endpoint);
      }
      "unregister_stdio" => {
        let endpoint = payload.get::<String>("endpoint")?;
        self.stdio_endpoints.remove(&endpoint);
      }
      "send" => {
        let endpoint = payload.get::<String>("endpoint")?;
        let name: Option<String> = payload
          .0
          .get("name")
          .and_then(|v| v.as_str())
          .map(|s| s.to_string());
        let branch = payload
          .0
          .get("branch")
          .and_then(|v| serde_json::from_value::<FlowMatchOperation>(v.clone()).ok()); // FIX: clone?
        let flow_payload = payload.0.get("payload").cloned();
        let action_str: String = payload
          .0
          .get("action")
          .and_then(|v| v.as_str())
          .map(|s| s.to_string())
          .unwrap_or("set".into());
        let type_str: String = payload
          .0
          .get("type")
          .and_then(|v| v.as_str())
          .map(|s| s.to_string())
          .unwrap_or("state".into());

        let msg = TransportMessage {
          r#type: if type_str == "signal" {
            TransportMessageType::Signal
          } else {
            TransportMessageType::State
          },
          payload: flow_payload.map(|v| FlowPayload::from_json(Some(v))),
          name,
          action: if action_str == "remove" {
            TransportMessageAction::Remove
          } else {
            TransportMessageAction::Set
          },
          branch,
        };
        if self.stdio_endpoints.contains(&endpoint) {
          let _ = dispatch.dispatch(
            "services",
            "send_stdio",
            serde_json::json!({
              "endpoint": endpoint,
              "message": msg
            })
            .into(),
          );
        } else {
          self.uds.send_message(&endpoint, &msg);
        }
      }
      "drain_incoming" => {
        if let Ok(rx) = self.uds.incoming_rx.lock() {
          while let Ok((_endpoint, msg)) = rx.try_recv() {
            match msg.r#type {
              TransportMessageType::State => {
                if let Some(name) = &msg.name {
                  if msg.action == TransportMessageAction::Remove {
                    let mut payload = serde_json::json!({ "name": name });
                    if let Some(p) = &msg.payload {
                      payload["filter"] = p.to_json();
                    }
                    let _ = dispatch.dispatch("flow", "remove_state", payload.into());
                  } else if msg.action == TransportMessageAction::Set {
                    let mut payload = serde_json::json!({ "name": name });
                    if let Some(p) = &msg.payload {
                      payload["payload"] = p.to_json();
                    }
                    let _ = dispatch.dispatch("flow", "set_state", payload.into());
                  }
                }
              }
              TransportMessageType::Signal => {
                if let Some(name) = &msg.name {
                  let mut payload = serde_json::json!({ "name": name });
                  if let Some(p) = &msg.payload {
                    payload["payload"] = p.to_json();
                  }
                  let _ = dispatch.dispatch("flow", "emit_signal", payload.into());
                }
              }
              _ => {}
            }
          }
        }
      }
      _ => {}
    }
    Ok(())
  }
}
