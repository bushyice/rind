use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::{
  flow::{FlowInstance, FlowMatchOperation, FlowPayload},
  services::Service,
  store::Store,
};

#[derive(Default, Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub struct TransportID(pub String);

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
  pub remove: bool,

  stop: bool,
}

impl<'a> TransportContext<'a> {
  pub fn new(input: &'a FlowPayload, except: &'a Vec<String>, remove: bool) -> Self {
    Self {
      input,
      except,
      remove,
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
  Lazy::new(|| std::sync::RwLock::new(HashMap::default()));

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
        self
          .emit_signal(msg.name.unwrap(), msg.payload, Some(&exceptions))
          .unwrap();
      }
      TransportMessageType::State => {
        if msg.action == TransportMessageAction::Remove {
          self.remove_state(
            &msg.name.unwrap(),
            if msg.payload.is_some() {
              match msg.payload.unwrap() {
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
          self
            .set_state(msg.name.unwrap(), msg.payload, Some(&exceptions))
            .unwrap();
        }
      }
    }
  }
}
