use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::{
  flow::{
    FlowInstance, FlowMatchOperation, FlowPayload, transports::stdio::StdioTransportProtocol,
    transports::uds::UdsTransportProtocol,
  },
  services::Service,
  store::Store,
};

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

pub trait TransportProtocol: Send + Sync {
  fn init(&mut self, _options: Vec<String>, service: Option<&mut crate::services::Service>);

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
