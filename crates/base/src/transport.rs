// State perms are impl'd partially for UDS connections and states, BUT.
// - have not been impl'd for stdio
// - probs more things i didn't think about

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use rind_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::flow::{FlowMatchOperation, FlowPayload, Signal, State};
use rind_core::notifier::Notifier;

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
  pub name: Option<Ustr>,
  #[serde(default)]
  pub action: TransportMessageAction,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct TransportProtocolId(pub Ustr);

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum TransportMethod {
  Type(TransportProtocolId),
  Options {
    id: TransportProtocolId,
    options: Vec<Ustr>,
    permissions: Option<Vec<Ustr>>,
  },
  Object {
    id: TransportProtocolId,
    options: serde_json::Value,
    permissions: Option<Vec<Ustr>>,
  },
}

impl TransportMethod {
  pub fn get_permissions(&self) -> Option<Vec<Ustr>> {
    match self.clone() {
      TransportMethod::Options {
        id: _,
        options: _,
        permissions,
      } => permissions,
      TransportMethod::Object {
        id: _,
        options: _,
        permissions,
      } => permissions,
      _ => None,
    }
  }
}

pub trait TransportProtocol: Send + Sync {
  fn setup(
    &mut self,
    endpoint: &str,
    permissions: Option<Vec<Ustr>>,
    pm: Option<PermissionStore>,
    notifier: Option<Notifier>,
  );
  fn send_message(&self, endpoint: &str, msg: &TransportMessage);
}

type ClientMap = Arc<Mutex<HashMap<Ustr, Vec<UnixStream>>>>;

fn socket_path(endpoint: &str) -> std::path::PathBuf {
  std::path::PathBuf::from("/run/rind-tp").join(format!("{endpoint}.sock"))
}

pub struct UdsTransport {
  clients: ClientMap,
  started: std::collections::HashSet<Ustr>,
  incoming_tx: std::sync::mpsc::Sender<(Ustr, TransportMessage, u32)>,
  incoming_rx: Arc<Mutex<std::sync::mpsc::Receiver<(Ustr, TransportMessage, u32)>>>,
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
  fn start_listener(
    &self,
    endpoint: Ustr,
    permissions: Option<Vec<Ustr>>,
    pm: Option<PermissionStore>,
    notifier: Option<Notifier>,
  ) {
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

    if permissions.is_some() {
      std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
        .expect("failed to allow permissions");
    }

    let clients = self.clients.clone();
    let tx = self.incoming_tx.clone();
    let ep = endpoint.clone();

    std::thread::spawn(move || {
      for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut uid = 0;

        if let Some(ref permissions) = permissions
          && let Some(ref pm) = pm
        {
          let Ok(cred) = get_peer_cred_stream(&stream) else {
            continue;
          };
          uid = cred.uid;

          if !permissions
            .iter()
            .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(cred.uid, x)))
          {
            drop(stream);
            continue;
          }
        }

        if let Ok(writer) = stream.try_clone() {
          if let Ok(mut locked) = clients.lock() {
            locked.entry(ep.clone()).or_default().push(writer);
          }
        }

        let tx = tx.clone();
        let ep_for_msg = ep.clone();
        let notifier = notifier.clone();
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
              let _ = tx.send((ep_for_msg.clone(), msg, uid));
              if let Some(n) = &notifier {
                let _ = n.notify();
              }
            }
          }
        });
      }
    });
  }
}

impl TransportProtocol for UdsTransport {
  fn setup(
    &mut self,
    endpoint: &str,
    permissions: Option<Vec<Ustr>>,
    pm: Option<PermissionStore>,
    notifier: Option<Notifier>,
  ) {
    let endpoint = Ustr::from(endpoint);
    if self.started.contains(&endpoint) {
      return;
    }
    self.start_listener(endpoint.clone(), permissions, pm, notifier);
    self.started.insert(endpoint);
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
  service_name: Ustr,
  child: &mut std::process::Child,
  tx: std::sync::mpsc::Sender<(Ustr, TransportMessage)>,
  notifier: Option<Notifier>,
) {
  if let Some(stdout) = child.stdout.take() {
    std::thread::spawn(move || {
      let reader = BufReader::new(stdout);
      for line in reader.lines().flatten() {
        if let Ok(msg) = serde_json::from_str::<TransportMessage>(&line) {
          let _ = tx.send((service_name.clone(), msg));
          if let Some(n) = &notifier {
            let _ = n.notify();
          }
        }
      }
    });
  }
}

pub struct TransportRuntime {
  uds: UdsTransport,
  stdio_endpoints: std::collections::HashSet<Ustr>,
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
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    let pm = ctx
      .scope
      .get::<PermissionStore>()
      .cloned()
      .unwrap_or_default();

    match action {
      "setup_uds" => {
        let endpoint = payload.get::<Ustr>("endpoint")?;
        let permissions = payload.get::<Vec<Ustr>>("permissions").ok();

        self.uds.setup(
          endpoint.as_str(),
          permissions,
          Some(pm),
          ctx.notifier.clone(),
        );
      }
      "register_stdio" => {
        let endpoint = payload.get::<Ustr>("endpoint")?;
        self.stdio_endpoints.insert(endpoint);
      }
      "unregister_stdio" => {
        let endpoint = payload.get::<Ustr>("endpoint")?;
        self.stdio_endpoints.remove(endpoint.as_str());
      }
      "send" => {
        let endpoint = payload.get::<Ustr>("endpoint")?;
        let name = payload.get::<Ustr>("name").ok();
        let branch = payload.get::<FlowMatchOperation>("branch").ok();
        let flow_payload = payload.get::<serde_json::Value>("payload").ok();
        let action_str: String = payload.get::<String>("action").ok().unwrap_or("set".into());
        let type_str: String = payload.get::<String>("type").ok().unwrap_or("state".into());

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
        // TODO: Add an option for more transport protocols
        if self.stdio_endpoints.contains(&endpoint) {
          let _ = dispatch.dispatch(
            "services",
            "send_stdio",
            rpayload!({
              "endpoint": endpoint.to_string(),
              "message": msg
            }),
          );
        } else {
          self.uds.send_message(endpoint.as_str(), &msg);
        }
      }
      "drain_incoming" => {
        if let Ok(rx) = self.uds.incoming_rx.lock() {
          while let Ok((_endpoint, msg, uid)) = rx.try_recv() {
            match msg.r#type {
              TransportMessageType::State => {
                if let Some(name) = &msg.name {
                  if uid != 0 {
                    // one-shot user-specific state defs
                    if let Some((username, _)) = name.as_str().split_once("/")
                      && let Some(user) = pm.users.lookup_by_uid(uid)
                      && username != user.username.as_str()
                    {
                      continue;
                    }

                    // one-shot perms (is this good?)
                    if let Some(state) = ctx.registry.metadata.find::<State>("units", name.as_str())
                      && let Some(perms) = &state.permissions
                      && !perms
                        .iter()
                        .any(|x| pm.from_name(x).map_or(false, |x| !pm.user_has(uid, x)))
                    {
                      continue;
                    }
                  }

                  let name = name.clone();

                  if msg.action == TransportMessageAction::Remove {
                    let mut payload = rpayload!({ "name": name });
                    if let Some(p) = &msg.payload {
                      payload = payload.insert("filter", p.to_json());
                    }
                    let _ = dispatch.dispatch("flow", "remove_state", payload);
                  } else if msg.action == TransportMessageAction::Set {
                    let mut payload = rpayload!({ "name": name });
                    if let Some(p) = &msg.payload {
                      payload = payload.insert("payload", p.to_json());
                    }
                    let _ = dispatch.dispatch("flow", "set_state", payload);
                  }
                }
              }
              TransportMessageType::Signal => {
                if let Some(name) = &msg.name {
                  // same with state perms, one-shot
                  if let Some(state) = ctx.registry.metadata.find::<Signal>("units", name.as_str())
                    && let Some(perms) = &state.permissions
                    && !perms
                      .iter()
                      .any(|x| pm.from_name(x).map_or(false, |x| !pm.user_has(uid, x)))
                  {
                    continue;
                  }

                  let name = name.clone();

                  let mut payload = rpayload!({ "name": name });
                  if let Some(p) = &msg.payload {
                    payload = payload.insert("payload", p.to_json());
                  }
                  let _ = dispatch.dispatch("flow", "emit_signal", payload);
                }
              }
              _ => {}
            }
          }
        }
      }
      _ => {}
    }
    Ok(None)
  }
}
