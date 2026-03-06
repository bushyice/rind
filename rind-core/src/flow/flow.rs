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

#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlowJson {
  inner: String,
}

impl FlowJson {
  pub fn into_json(&self) -> serde_json::Value {
    serde_json::from_str(&self.inner).unwrap()
  }

  pub fn to_string(&self) -> String {
    self.inner.clone()
  }

  pub fn swap(&mut self, value: serde_json::Value) {
    self.inner = value.to_string();
  }
}

impl From<String> for FlowJson {
  fn from(value: String) -> Self {
    Self { inner: value }
  }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FlowPayload {
  Json(FlowJson),
  String(String),
  Bytes(Vec<u8>),
  None(bool),
}

impl FlowPayload {
  pub fn to_string(&self) -> String {
    match self {
      FlowPayload::String(s) => s.clone(),
      FlowPayload::Json(s) => s.to_string(),
      // FIX: Proper error handling
      FlowPayload::Bytes(s) => String::from_utf8(s.clone()).unwrap_or("".to_string()),
      FlowPayload::None(_) => "".to_string(),
    }
  }

  pub fn contains(&self, contains: &String) -> bool {
    match self {
      FlowPayload::String(s) => s.contains(contains),
      FlowPayload::Json(s) => Self::value_to_vec_string(&s.into_json()).contains(contains),
      // TODO: Add a binary contains checker
      FlowPayload::Bytes(_) => false,
      FlowPayload::None(_) => false,
    }
  }

  pub fn value_to_vec_string(value: &serde_json::Value) -> Vec<String> {
    match value {
      serde_json::Value::Array(arr) => arr
        .into_iter()
        .filter_map(|v| match v {
          serde_json::Value::String(s) => Some(s.clone()),
          _ => None,
        })
        .collect(),
      _ => vec!["".to_string()],
    }
  }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowPayloadType {
  #[default]
  Json,
  String,
  Bytes,
  None,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct FlowDefinitionBase {
  pub name: String,
  pub payload: FlowPayloadType,
  pub broadcast: Option<Vec<String>>,
  pub branch: Option<Vec<String>>,
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
