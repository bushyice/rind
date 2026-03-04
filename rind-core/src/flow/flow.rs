use std::ops::Deref;

use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowItem {
  Simple(String),
  Detailed {
    state: Option<String>,
    signal: Option<String>,
    target: Option<FlowMatchOperation>,
    branch: Option<FlowMatchOperation>,
  },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowMatchOperation {
  Eq(String),
  Options {
    binary: Option<bool>,
    contains: Option<String>,
    r#as: Option<serde_json::Value>,
    // Optional addition for searchers here
  },
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FlowType {
  #[default]
  Signal,
  State,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FlowInstance {
  pub name: String,
  pub payload: FlowPayload,

  #[serde(skip)]
  pub r#type: FlowType,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FlowPayload {
  Json(serde_json::Value),
  String(String),
  Bytes(Vec<u8>),
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowPayloadType {
  #[default]
  Json,
  String,
  Bytes,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct FlowDefinitionBase {
  pub name: String,
  pub payload: FlowPayloadType,
  pub broadcast: Option<Vec<String>>,
  // pub permission: Option<Permission>
  pub after: Option<Vec<FlowItem>>,
  pub subscribers: Option<Vec<TransportMethod>>,
  pub trigger: Option<Vec<Trigger>>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StateDefinition(pub FlowDefinitionBase);
impl Deref for StateDefinition {
  type Target = FlowDefinitionBase;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SignalDefinition(pub FlowDefinitionBase);
impl Deref for SignalDefinition {
  type Target = FlowDefinitionBase;
  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

pub struct FlowOutput {
  pub input: FlowPayload,
  pub outputs: Vec<FlowPayload>,
}
