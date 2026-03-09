use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::{
  flow::{
    FlowInstance, FlowMatchOperation, FlowPayload, FlowType, SignalDefinition, StateDefinition,
    transports::args::ArgsTransportProtocol, transports::env::EnvTransportProtocol,
    transports::stdio::StdioTransportProtocol, transports::uds::UdsTransportProtocol,
  },
  services::Service,
  store::Store,
};
use rind_common::error::{report_error, rw_write};

#[derive(Default, Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct TransportID(pub String);
impl From<String> for TransportID {
  fn from(value: String) -> Self {
    TransportID(value)
  }
}
impl From<&str> for TransportID {
  fn from(value: &str) -> Self {
    TransportID(value.to_string())
  }
}

pub type SubscriberID = u64;

pub type TransportError = anyhow::Error;
pub type TransportResult = Result<Option<FlowPayload>, TransportError>;

pub type TransportSubscriber = dyn FnMut(&mut FlowInstance) -> TransportResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportInitStage {
  DetachedBoot,
  ServicePreStart,
  ServicePostStart,
}

pub struct TransportInitContext<'a> {
  pub endpoint: &'a str,
  pub flow_type: FlowType,
  pub detached: bool,
  pub stage: TransportInitStage,
}

pub trait TransportProtocol: Send + Sync {
  fn init(
    &mut self,
    _options: Vec<String>,
    _ctx: &TransportInitContext,
    _service: Option<&mut crate::services::Service>,
  );

  fn recv(
    &self,
    ctx: &mut TransportContext,
    instance: &FlowInstance,
    service: Option<&mut Service>,
  ) -> TransportResult;
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum TransportMethod {
  Simple(TransportID),
  Options {
    id: TransportID,
    options: Vec<String>,
  },
}

impl TransportMethod {
  pub fn as_id(&self) -> &TransportID {
    match self {
      TransportMethod::Simple(id) => &id,
      TransportMethod::Options { id, options: _ } => &id,
    }
  }

  pub fn options(&self) -> Vec<String> {
    match self {
      TransportMethod::Simple(_) => Vec::new(),
      TransportMethod::Options { id: _, options } => options.clone(),
    }
  }
}

pub struct TransportContext<'a> {
  records: Vec<FlowPayload>,
  pub input: &'a FlowPayload,

  pub except: &'a Vec<String>,
  pub action: TransportMessageAction,

  stop: bool,
}

impl<'a> TransportContext<'a> {
  pub fn new(
    input: &'a FlowPayload,
    except: &'a Vec<String>,
    action: TransportMessageAction,
  ) -> Self {
    Self {
      input,
      except,
      action,
      records: Vec::new(),
      stop: false,
    }
  }

  pub fn stop(&mut self) {
    self.stop = true;
  }

  pub fn stopped(&self) -> bool {
    self.stop
  }

  pub fn records(&self) -> impl Iterator<Item = &FlowPayload> {
    self.records.iter()
  }

  pub fn clear_records(&mut self) {
    self.records.clear();
  }
}

pub static TRANSPORTS: Lazy<std::sync::RwLock<HashMap<TransportID, Box<dyn TransportProtocol>>>> =
  Lazy::new(|| {
    let mut transports: HashMap<TransportID, Box<dyn TransportProtocol>> = HashMap::default();

    transports.insert("stdio".into(), Box::new(StdioTransportProtocol));
    transports.insert("uds".into(), Box::new(UdsTransportProtocol::default()));
    transports.insert("env".into(), Box::new(EnvTransportProtocol));
    transports.insert("args".into(), Box::new(ArgsTransportProtocol));

    std::sync::RwLock::new(transports)
  });

#[derive(Serialize, Deserialize, Copy, Clone)]
pub enum TransportMessageType {
  Signal,
  State,
  Enquiry,
  Respose,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Copy, Clone)]
pub enum TransportMessageAction {
  #[default]
  Set,
  Remove,
}

#[derive(Serialize, Deserialize)]
pub struct TransportMessage {
  pub r#type: TransportMessageType,
  pub payload: Option<FlowPayload>,
  pub name: Option<String>,
  #[serde(default)]
  pub action: TransportMessageAction,
}

impl Store {
  pub fn init_detached_transports(&self) {
    let mut entries: Vec<(String, FlowType, TransportMethod)> = Vec::new();

    for (unit, state) in self.items::<StateDefinition>() {
      if let Some(subscribers) = &state.subscribers {
        let endpoint = format!("{}@{}", unit.to_string(), state.name);
        for method in subscribers {
          entries.push((endpoint.clone(), FlowType::State, method.clone()));
        }
      }
    }

    for (unit, signal) in self.items::<SignalDefinition>() {
      if let Some(subscribers) = &signal.subscribers {
        let endpoint = format!("{}@{}", unit.to_string(), signal.name);
        for method in subscribers {
          entries.push((endpoint.clone(), FlowType::Signal, method.clone()));
        }
      }
    }

    for (endpoint, flow_type, method) in entries {
      init_transport_method(
        &method,
        &endpoint,
        flow_type,
        true,
        TransportInitStage::DetachedBoot,
        None,
      );
    }
  }

  pub fn handle_message(&mut self, service_name: String, msg: TransportMessage) {
    let exceptions = vec![service_name.clone()];

    match msg.r#type {
      TransportMessageType::Enquiry => {}
      TransportMessageType::Respose => {}
      TransportMessageType::Signal => {
        let Some(name) = msg.name else {
          return;
        };
        let _ = self.emit_signal(name, msg.payload, Some(&exceptions));
      }
      TransportMessageType::State => {
        if msg.action == TransportMessageAction::Remove {
          let Some(name) = msg.name else {
            return;
          };
          self.remove_state(
            &name,
            if msg.payload.is_some() {
              match msg.payload.unwrap_or(FlowPayload::None(false)) {
                FlowPayload::Json(i) => Some(FlowMatchOperation::Options {
                  binary: None,
                  contains: None,
                  r#as: Some(i.into_json()),
                }),
                FlowPayload::Bytes(_) => None,
                FlowPayload::String(i) => Some(FlowMatchOperation::Eq(i)),
                FlowPayload::None(_) => None,
              }
            } else {
              None
            },
            Some(&exceptions),
          );
        } else {
          let Some(name) = msg.name else {
            return;
          };
          let _ = self.set_state(name, msg.payload, Some(&exceptions));
        }
      }
    }
  }
}

pub fn init_service_transport(service: &mut Service, stage: TransportInitStage) {
  let Some(method) = service.transport.clone() else {
    return;
  };
  if method.as_id().0.trim().is_empty() {
    return;
  }
  let endpoint = format!("{}@{}", service.unit.to_string(), service.name);
  init_transport_method(
    &method,
    &endpoint,
    FlowType::State,
    false,
    stage,
    Some(service),
  );
}

fn init_transport_method(
  method: &TransportMethod,
  endpoint: &str,
  flow_type: FlowType,
  detached: bool,
  stage: TransportInitStage,
  mut service: Option<&mut Service>,
) {
  let id = method.as_id().0.as_str();
  if detached && id == "stdio" {
    report_error("transport init skipped", "stdio is service-only");
    return;
  }
  if detached && (id == "env" || id == "args") {
    report_error("transport init skipped", format!("{id} is service-only"));
    return;
  }
  if flow_type == FlowType::Signal && (id == "env" || id == "args") {
    report_error("transport init skipped", format!("{id} is state-only"));
    return;
  }
  if (id == "env" || id == "args") && !matches!(stage, TransportInitStage::ServicePreStart) {
    return;
  }
  if id == "stdio" && !matches!(stage, TransportInitStage::ServicePostStart) {
    return;
  }

  let mut transports = rw_write(&TRANSPORTS, "transports write in init_transport_method");
  let Some(protocol) = transports.get_mut(method.as_id()) else {
    report_error(
      "transport init failed",
      format!("missing transport id: {}", method.as_id().0),
    );
    return;
  };
  let ctx = TransportInitContext {
    endpoint,
    flow_type,
    detached,
    stage,
  };
  protocol.init(method.options(), &ctx, service.as_deref_mut());
}
