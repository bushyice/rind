use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

use crate::flow::{FlowInstance, FlowPayload};

#[derive(Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportID(pub String);

pub type SubscriberID = u64;

bitflags::bitflags! {
  pub struct TransportCapabilities: u8 {
    const INPUT  = 0b00000001;
    const OUTPUT = 0b00000010;
  }
}

type TransportError = anyhow::Error;
type TransportResult = Result<Option<FlowPayload>, TransportError>;

pub type TransportSubscriber = dyn FnMut(&mut FlowInstance) -> TransportResult;

pub trait TransportProtocolInput: Send + Sync {
  fn incoming(&self, ctx: &mut TransportContext, payload: &mut FlowInstance) -> TransportResult;
}
pub trait TransportProtocolOutput: Send + Sync {
  fn outgoing(&self, ctx: &mut TransportContext, payload: &mut FlowInstance) -> TransportResult;
}

pub trait Subscriber: Send + Sync {
  fn subscribe(&self, _name: String, _sub: Box<TransportSubscriber>) -> SubscriberID {
    0
  }
  fn unsubscribe(&self, _sub: SubscriberID) {}
}

pub trait TransportProtocol: TransportProtocolInput + TransportProtocolOutput + Subscriber {
  fn caps(&self) -> TransportCapabilities;
  fn init(&mut self, _options: Vec<String>);
}

pub static TRANSPORTS: Lazy<std::sync::RwLock<HashMap<TransportID, Box<dyn TransportProtocol>>>> =
  Lazy::new(|| std::sync::RwLock::new(HashMap::default()));

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TransportMethod {
  Simple(TransportID),
  Options {
    id: TransportID,
    options: Vec<String>,
  },
}

#[derive(Default)]
pub struct TransportContext {
  records: Vec<FlowPayload>,

  stop: bool,
}

impl TransportContext {
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
