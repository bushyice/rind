// State perms are impl'd partially for UDS connections and states, BUT.
// - have not been impl'd for stdio
// - probs more things i didn't think about

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use crate::flow::{Signal, State};
use crate::prelude::Service;
use rind_core::notifier::Notifier;
use rind_core::prelude::*;
pub use rind_ipc::{
  FlowMatchOperation, FlowPayload, TransportMessage, TransportMessageAction, TransportMessageType,
};
use serde::{Deserialize, Serialize};
use serde_json;
use tachyon_ipc::Bus;

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
  pub clients: ClientMap,
  pub started: std::collections::HashSet<Ustr>,
  pub incoming_tx: std::sync::mpsc::Sender<(Ustr, TransportMessage, u32)>,
  pub incoming_rx: Arc<Mutex<std::sync::mpsc::Receiver<(Ustr, TransportMessage, u32)>>>,
}

pub struct TachyonTransport {
  pub clients: Arc<Mutex<HashMap<Ustr, Vec<Arc<Mutex<Bus>>>>>>,
  pub started: std::collections::HashSet<Ustr>,
  pub incoming_tx: std::sync::mpsc::Sender<(Ustr, TransportMessage, u32)>,
  pub incoming_rx: Arc<Mutex<std::sync::mpsc::Receiver<(Ustr, TransportMessage, u32)>>>,
}

impl Default for TachyonTransport {
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

fn tachyon_path(endpoint: &str) -> String {
  let base = std::env::var("RIND_TP_DIR").unwrap_or_else(|_| "/run/rind-tp".to_string());
  format!("{}/{}.tachyon", base, endpoint)
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
          let mut reader = stream;
          loop {
            match TransportMessage::read_signed(&mut reader) {
              Ok(msg) => {
                let _ = tx.send((ep_for_msg.clone(), msg, uid));
                if let Some(n) = &notifier {
                  let _ = n.notify();
                }
              }
              Err(_) => break,
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
    if let Ok(mut locked) = self.clients.lock() {
      if let Some(clients) = locked.get_mut(endpoint) {
        clients.retain_mut(|client| msg.write_signed(client).is_ok());
      }
    }
  }
}

impl TachyonTransport {
  fn start_listener(
    &self,
    endpoint: Ustr,
    _permissions: Option<Vec<Ustr>>,
    _pm: Option<PermissionStore>,
    notifier: Option<Notifier>,
  ) {
    let path = tachyon_path(&endpoint);
    let clients = self.clients.clone();
    let tx = self.incoming_tx.clone();
    let ep = endpoint.clone();

    std::thread::spawn(move || {
      loop {
        let bus = match Bus::listen(&path, 1 << 20) {
          Ok(b) => Arc::new(Mutex::new(b)),
          Err(e) => {
            eprintln!("[transport] tachyon listen failed: {e:?}");
            // Optional: sleep and retry
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
          }
        };

        {
          let mut locked = clients.lock().unwrap();
          locked.entry(ep.clone()).or_default().push(bus.clone());
        }

        let tx = tx.clone();
        let ep_inner = ep.clone();
        let notifier = notifier.clone();
        let bus_inner = bus.clone();
        std::thread::spawn(move || {
          loop {
            // TODO: lock fix
            let bus = bus_inner.lock().unwrap();
            let guard = match bus.acquire_rx(10_000) {
              Ok(g) => g,
              Err(_) => break,
            };
            if let Ok(msg) = flexbuffers::from_slice::<TransportMessage>(guard.data()) {
              let _ = tx.send((ep_inner.clone(), msg, 0));
              if let Some(n) = &notifier {
                let _ = n.notify();
              }
            }
            let _ = guard.commit();
          }
        });
      }
    });
  }
}

impl TransportProtocol for TachyonTransport {
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
    let frame = flexbuffers::to_vec(msg).unwrap_or_default();
    if let Ok(mut locked) = self.clients.lock() {
      if let Some(buses) = locked.get_mut(endpoint) {
        buses.retain(|bus| {
          let locked_bus = bus.lock().unwrap();
          locked_bus.send(&frame, 0).is_ok()
        });
      }
    }
  }
}

pub fn start_stdout_listener(
  service_name: Ustr,
  stdout: Option<Box<dyn std::io::Read + Send>>,
  tx: std::sync::mpsc::Sender<(Ustr, TransportMessage, usize)>,
  notifier: Option<Notifier>,
  index: usize,
) {
  if let Some(mut stdout) = stdout {
    std::thread::spawn(move || {
      loop {
        match TransportMessage::read_signed(&mut stdout) {
          Ok(msg) => {
            let _ = tx.send((service_name.clone(), msg, index));
            if let Some(n) = &notifier {
              let _ = n.notify();
            }
          }
          Err(e) => {
            let _ = tx.send((
              service_name.clone(),
              TransportMessage::log(format!("failed to recieve message: {e}")),
              index,
            ));
            if let Some(n) = &notifier {
              let _ = n.notify();
            }
            break;
          }
        }
      }
    });
  }
}

pub struct TransportRuntime {
  pub uds: UdsTransport,
  pub tachyon: TachyonTransport,
  pub stdio_endpoints: std::collections::HashSet<Ustr>,
}

impl TransportRuntime {
  fn ingest(
    &self,
    _endpoint: Ustr,
    msg: TransportMessage,
    uid: u32,
    dispatch: &RuntimeDispatcher,
    pm: &PermissionStore,
    ctx: &mut RuntimeContext<'_>,
  ) -> CoreResult<()> {
    match msg.r#type {
      TransportMessageType::State => {
        if let Some(name) = &msg.name {
          if uid != 0 {
            // one-shot user-specific state defs
            if let Some((username, _)) = name.as_str().split_once("/")
              && let Some(user) = pm.users.lookup_by_uid(uid)
              && username != user.username.as_str()
            {
              return Ok(());
            }

            // one-shot perms (is this good?)
            if let Some(state) = ctx.registry.metadata.find::<State>("*", name.as_str())
              && let Some(perms) = &state.permissions
              && !perms
                .iter()
                .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
            {
              return Ok(());
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
          if let Some(state) = ctx.registry.metadata.find::<Signal>("*", name.as_str())
            && let Some(perms) = &state.permissions
            && !perms
              .iter()
              .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
          {
            return Ok(());
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

    Ok(())
  }
}

impl Default for TransportRuntime {
  fn default() -> Self {
    Self {
      uds: UdsTransport::default(),
      tachyon: TachyonTransport::default(),
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
      "setup_tachyon" => {
        let endpoint = payload.get::<Ustr>("endpoint")?;
        let permissions = payload.get::<Vec<Ustr>>("permissions").ok();

        self.tachyon.setup(
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
          self.tachyon.send_message(endpoint.as_str(), &msg);
        }
      }
      "ingest" => {
        let endpoint = payload.get::<Ustr>("endpoint")?;
        let index = payload.get::<usize>("index")?;
        let message = payload.get::<TransportMessage>("message")?;

        let child = ctx
          .registry
          .as_one::<Service>("*", endpoint.clone())?
          .instances
          .get(index)
          .ok_or(CoreError::Unknown)?;

        let uid = if let Some(user) = &child.user {
          pm.users
            .lookup_by_name(&user)
            .ok_or(CoreError::Unknown)?
            .uid
        } else {
          0
        };

        self.ingest(endpoint, message, uid, dispatch, &pm, ctx)?;
      }
      "drain_incoming" => {
        if let Ok(rx) = self.uds.incoming_rx.lock() {
          while let Ok((endpoint, msg, uid)) = rx.try_recv() {
            self.ingest(endpoint, msg, uid, dispatch, &pm, ctx)?;
          }
        }
        if let Ok(rx) = self.tachyon.incoming_rx.lock() {
          while let Ok((endpoint, msg, uid)) = rx.try_recv() {
            self.ingest(endpoint, msg, uid, dispatch, &pm, ctx)?;
          }
        }
      }
      _ => {}
    }
    Ok(None)
  }
}
