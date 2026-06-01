// TODO: Fix stuff
// - Add specific service-only transports
// - Group transports instead of per subscriber
// -
// TODO: Socket cleanups

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};

use crate::shm_tp::ShmClient;
use crate::{FacetGraph, FlowFacet, FlowImpulse};
use rind_core::notifier::Notifier;
use rind_core::prelude::*;
pub use rind_ipc::{
  FlowMatchOperation, FlowPayload, TransportMessage, TransportMessageAction, TransportMessageType,
};
use serde::{Deserialize, Serialize};
use serde_json;

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

#[model(
  meta_name = name,
  meta_fields(
    name, protocol
  ),
  derive_metadata(Debug, Clone)
)]
pub struct TransportRoute {
  pub name: Ustr,
  pub protocol: TransportMethod,
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

pub enum TransportResponder {
  Uds(UnixStream),
  Shm(Arc<ShmClient>),
}

impl TransportResponder {
  pub fn send(&mut self, msg: &TransportMessage) -> std::io::Result<Void> {
    match self {
      TransportResponder::Uds(stream) => msg.write_signed(stream),
      TransportResponder::Shm(stream) => stream
        .evt_to_client
        .write(1)
        .and_then(|_| {
          if stream.ring_to_client.write(&msg.as_bytes()) {
            Ok(Void)
          } else {
            Err(reexports::nix::errno::Errno::EBADF)
          }
        })
        .map_err(|x| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{x}"))),
    }
  }
}

type ClientMap = Arc<Mutex<HashMap<Ustr, Vec<UnixStream>>>>;

pub fn socket_path(endpoint: &str) -> std::path::PathBuf {
  std::path::PathBuf::from("/run/rind-tp").join(format!("{endpoint}.sock"))
}

pub struct UdsTransport {
  pub clients: ClientMap,
  pub started: std::collections::HashSet<Ustr>,
  pub incoming_tx:
    std::sync::mpsc::Sender<(Ustr, TransportMessage, u32, Option<TransportResponder>)>,
  pub incoming_rx: Arc<
    Mutex<std::sync::mpsc::Receiver<(Ustr, TransportMessage, u32, Option<TransportResponder>)>>,
  >,
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
                let responder = if matches!(msg.r#type, TransportMessageType::Enquiry) {
                  reader.try_clone().ok().map(TransportResponder::Uds)
                } else {
                  None
                };
                let _ = tx.send((ep_for_msg.clone(), msg, uid, responder));
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
            if matches!(
              e.kind(),
              std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionReset
            ) {
              // println!("broken pipe");
              break;
            }
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
  pub shm: crate::shm_tp::ShmTransport,
  pub stdio_endpoints: std::collections::HashSet<Ustr>,
}

impl TransportRuntime {
  fn handle_enquiry(
    &self,
    msg: &TransportMessage,
    _uid: u32,
    _pm: &PermissionStore,
    ctx: &mut RuntimeContext<'_>,
  ) -> CoreResult<TransportMessage> {
    let mut response = TransportMessage {
      r#type: TransportMessageType::Response,
      payload: None,
      name: msg.name.clone(),
      action: msg.action,
      branch: None,
    };

    if let Some(name) = &msg.name {
      match name.as_str() {
        "has_state" => {
          if let Some(payload) = &msg.payload {
            let state_name = payload.to_string_payload();
            let sm = ctx
              .registry
              .singleton::<FacetGraph>(FacetGraph::KEY)
              .ok_or_else(|| CoreError::InvalidState("state machine store not found".into()))?;
            let exists = sm.facets.contains_key(&Ustr::from(state_name.as_str()));
            response.payload = Some(FlowPayload::from_json(Some(serde_json::json!(exists))));
          }
        }
        _ => {
          response.payload = Some(FlowPayload::from_json(Some(serde_json::json!({
            "error": "unknown enquiry",
            "enquiry": name.as_str()
          }))));
        }
      }
    }

    Ok(response)
  }

  fn ingest(
    &self,
    endpoint: Ustr,
    msg: TransportMessage,
    uid: u32,
    dispatch: &RuntimeDispatcher,
    pm: &PermissionStore,
    ctx: &mut RuntimeContext<'_>,
    responder: Option<TransportResponder>,
    log: &LogHandle,
  ) -> CoreResult<Void> {
    if msg.name.as_ref().map(|x| x.as_str()) == Some("watchdog") {
      let _ = dispatch.dispatch(
        "services",
        "watchdog_ping",
        rpayload!({ "service": endpoint.clone() }),
      );
      return Ok(Void);
    }

    if msg.name.as_ref().map(|x| x.as_str()) == Some("log") {
      let service_name = rslvns!(snorm endpoint).to_ustr();
      let (level, message_text, fields) = stdio_log_entry(&service_name, &msg);
      log.log(level, "service-transport", message_text, fields);
      return Ok(Void);
    }

    match msg.r#type {
      TransportMessageType::Enquiry => {
        if let Some(mut responder) = responder {
          let response = self.handle_enquiry(&msg, uid, pm, ctx)?;
          let _ = responder.send(&response);
        }
      }
      TransportMessageType::Facet => {
        if let Some(name) = &msg.name {
          if uid != 0 {
            // one-shot user-specific state defs
            if let Some((username, _)) = name.as_str().split_once("/")
              && let Some(user) = pm.users.lookup_by_uid(uid)
              && username != user.username.as_str()
            {
              return Ok(Void);
            }

            // one-shot perms (is this good?)
            if let Some(state) = ctx.registry.metadata.find::<FlowFacet>("*", name.as_str())
              && let Some(perms) = &state.permissions
              && !perms
                .iter()
                .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
            {
              return Ok(Void);
            }
          }

          let name = name.clone();

          if msg.action == TransportMessageAction::Remove {
            let mut act = crate::FlowRuntime::actions.remove_facet(name);
            if let Some(p) = &msg.payload {
              act = act.payload(p.to_json());
            }
            let _ = act.dispatch(dispatch);
          } else if msg.action == TransportMessageAction::Set {
            let mut act = crate::FlowRuntime::actions.set_facet(name);
            if let Some(p) = &msg.payload {
              act = act.payload(p.to_json());
            }
            let _ = act.dispatch(dispatch);
          }
        }
      }
      TransportMessageType::Impulse => {
        if let Some(name) = &msg.name {
          // same with state perms, one-shot
          if let Some(state) = ctx
            .registry
            .metadata
            .find::<FlowImpulse>("*", name.as_str())
            && let Some(perms) = &state.permissions
            && !perms
              .iter()
              .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(uid, x)))
          {
            return Ok(Void);
          }

          let name = name.clone();

          let mut act = crate::FlowRuntime::actions.impulse(name);
          if let Some(p) = &msg.payload {
            act = act.payload(p.to_json());
          }
          let _ = act.dispatch(dispatch);
        }
      }
      _ => {}
    }

    Ok(Void)
  }

  fn scope_from_route(name: &str) -> Ustr {
    let mut parts = name.rsplitn(2, '@');
    let scope = parts.next().unwrap_or("static");
    let left = parts.next();
    if left.is_some() {
      Ustr::from(scope)
    } else {
      Ustr::from("static")
    }
  }
}

impl Default for TransportRuntime {
  fn default() -> Self {
    Self {
      uds: UdsTransport::default(),
      shm: crate::shm_tp::ShmTransport::default(),
      stdio_endpoints: std::collections::HashSet::new(),
    }
  }
}

#[runtime("transport")]
impl TransportRuntime {
  fn bootstrap(&mut self) {
    for (group, tp) in ctx
      .registry
      .metadata
      .items::<TransportRoute>("static")
      .unwrap_or_default()
    {
      let endpoint = rslvns!(group, tp.name);
      let id = transport_id(&tp.protocol);
      if id == "uds" || id == "shm" {
        if id == "uds" {
          let mut payload = rpayload!({
            "endpoint": endpoint.to_ustr(),
          });
          if let Some(perms) = tp.protocol.get_permissions() {
            payload = payload.insert("permissions", perms);
          }
          self.__runtime_setup_uds(payload, ctx, dispatch, log)?;
        } else {
          let mut payload = rpayload!({
            "endpoint": endpoint.to_ustr(),
          });
          if let Some(perms) = tp.protocol.get_permissions() {
            payload = payload.insert("permissions", perms);
          }
          self.__runtime_setup_shm(payload, ctx, dispatch, log)?;
        }
      }
    }
  }

  fn setup_route(&mut self, endpoint: Ustr) {
    let endpoint = endpoint.trim_start_matches("route:");
    let scope = Self::scope_from_route(endpoint);

    let route = ctx
      .registry
      .metadata
      .lookup::<TransportRoute>(scope, endpoint.to_ustr())
      .ok_or(CoreError::not_found("transport route", endpoint))?;

    setup_transport_endpoint(dispatch, endpoint, &route.protocol);
  }

  fn setup_uds(&mut self, endpoint: Ustr, #[optional] permissions: Vec<Ustr>) {
    let pm = ctx
      .scope
      .get::<PermissionStore>()
      .cloned()
      .unwrap_or_default();

    self.uds.setup(
      endpoint.as_str(),
      permissions,
      Some(pm),
      ctx.notifier.clone(),
    );
  }

  fn setup_shm(&mut self, endpoint: Ustr, #[optional] permissions: Vec<Ustr>) {
    let pm = ctx
      .scope
      .get::<PermissionStore>()
      .cloned()
      .unwrap_or_default();

    self.shm.setup(
      endpoint.as_str(),
      permissions,
      Some(pm),
      ctx.notifier.clone(),
    );
  }

  fn register_stdio(&mut self, endpoint: Ustr) {
    self.stdio_endpoints.insert(endpoint);
  }

  fn unregister_stdio(&mut self, endpoint: Ustr) {
    self.stdio_endpoints.remove(endpoint.as_str());
  }

  fn send(
    &mut self,
    endpoint: Ustr,
    #[optional] name: Ustr,
    #[optional] branch: FlowMatchOperation,
    #[optional] action: String,
    #[optional] r#type: String,
    #[optional] payload: serde_json::Value,
  ) {
    let action_str = action.unwrap_or_else(|| "set".into());
    let type_str = r#type.unwrap_or_else(|| "facet".into());

    let msg = TransportMessage {
      r#type: if type_str == "signal" {
        TransportMessageType::Impulse
      } else {
        TransportMessageType::Facet
      },
      payload: payload.map(|v| FlowPayload::from_json(Some(v))),
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
        rpayload!({
          "endpoint": endpoint.to_string(),
          "message": msg.clone()
        }),
      );
    } else {
      self.uds.send_message(endpoint.as_str(), &msg);
      self.shm.send_message(endpoint.as_str(), &msg);
    }
  }

  fn ingest(&mut self, endpoint: Ustr, uid: u32, message: TransportMessage) {
    let pm = ctx
      .scope
      .get::<PermissionStore>()
      .cloned()
      .unwrap_or_default();

    self.ingest(endpoint, message, uid, dispatch, &pm, ctx, None, log)?;
  }

  fn drain_incoming(&mut self) {
    let pm = ctx
      .scope
      .get::<PermissionStore>()
      .cloned()
      .unwrap_or_default();

    if let Ok(rx) = self.uds.incoming_rx.lock() {
      while let Ok((endpoint, msg, uid, responder)) = rx.try_recv() {
        self.ingest(endpoint, msg, uid, dispatch, &pm, ctx, responder, log)?;
      }
    }
    if let Ok(rx) = self.shm.incoming_rx.lock() {
      while let Ok((endpoint, msg, uid, responder)) = rx.try_recv() {
        self.ingest(endpoint, msg, uid, dispatch, &pm, ctx, responder, log)?;
      }
    }
  }
}

pub fn stdio_log_entry(
  service_name: &str,
  message: &TransportMessage,
) -> (LogLevel, String, HashMap<String, String>) {
  let mut level = LogLevel::Info;
  let mut text = String::new();
  let mut fields = HashMap::new();
  fields.insert("service".to_string(), service_name.to_string());
  fields.insert("source".to_string(), "stdio".to_string());

  if let Some(payload) = message.payload.as_ref() {
    match payload {
      FlowPayload::String(s) => {
        text = s.clone();
      }
      FlowPayload::Bytes(b) => {
        text = String::from_utf8(b.clone()).unwrap_or_default();
      }
      FlowPayload::Json(json) => {
        let value = json.into_json();
        if let Some(s) = value.get("message").and_then(|v| v.as_str()) {
          text = s.to_string();
        } else {
          text = value.to_string();
        }

        if let Some(lvl) = value.get("level").and_then(|v| v.as_str()) {
          level = parse_log_level(lvl);
        }

        if let Some(extra) = value.get("fields").and_then(|v| v.as_object()) {
          for (k, v) in extra {
            let val = v
              .as_str()
              .map(|s| s.to_string())
              .unwrap_or_else(|| v.to_string());
            fields.insert(k.clone(), val);
          }
        }
      }
      FlowPayload::None(_) => {}
    }
  }

  if text.is_empty() {
    text = "log".to_string();
  }

  (level, text, fields)
}

fn parse_log_level(input: &str) -> LogLevel {
  match input.to_ascii_lowercase().as_str() {
    "trace" => LogLevel::Trace,
    "debug" => LogLevel::Debug,
    "warn" | "warning" => LogLevel::Warn,
    "error" => LogLevel::Error,
    "fatal" => LogLevel::Fatal,
    _ => LogLevel::Info,
  }
}

pub fn transport_id<'a>(transport: &'a TransportMethod) -> &'a str {
  match transport {
    TransportMethod::Type(id) => id.0.as_str(),
    TransportMethod::Options { id, .. } => id.0.as_str(),
    TransportMethod::Object { id, .. } => id.0.as_str(),
  }
}

pub fn transport_endpoint<'a>(transport: &'a TransportMethod) -> Option<&'a str> {
  match transport {
    TransportMethod::Options { options, .. } => options.get(0).and_then(|x| {
      if x.starts_with("addr:") {
        Some(x.trim_start_matches("addr:"))
      } else {
        None
      }
    }),
    TransportMethod::Object { .. } => None,
    _ => None,
  }
}

pub fn setup_transport_endpoint(
  dispatch: &RuntimeDispatcher,
  endpoint: &str,
  transport: &TransportMethod,
) {
  let id = transport_id(transport);
  let endpoint = transport_endpoint(transport).unwrap_or(endpoint);

  if id == "uds" || id == "shm" {
    if id == "uds" {
      let mut act = TransportRuntime::actions.setup_uds(endpoint.to_ustr());
      if let Some(perms) = transport.get_permissions() {
        act = act.permissions(perms);
      }
      let _ = act.dispatch(dispatch);
    } else {
      let mut act = TransportRuntime::actions.setup_shm(endpoint.to_ustr());
      if let Some(perms) = transport.get_permissions() {
        act = act.permissions(perms);
      }
      let _ = act.dispatch(dispatch);
    }
  } else if id.starts_with("route:") {
    let _ = TransportRuntime::actions
      .setup_route(id.to_ustr())
      .dispatch(dispatch);
  }
}
