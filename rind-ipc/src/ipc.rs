#[cfg(feature = "server")]
pub mod recv;
pub mod send;
pub mod ser;

#[cfg(feature = "server")]
pub use rind_core::services::{Service, ServiceState};

pub use rind_common::UnitType;

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum MessageType {
  List,
  Start,
  Enable,
  Disable,
  Stop,
  Error,
  Unknown,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Message {
  pub r#type: MessageType,
  pub payload: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Payload {
  pub name: String,
  pub unit_type: UnitType,
  pub force: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ArrayPayload<T> {
  items: Vec<T>,
}

impl Message {
  pub fn from_type(t: MessageType) -> Self {
    Self {
      r#type: t,
      payload: None,
    }
  }

  pub fn with(mut self, payload: String) -> Self {
    self.payload = Some(payload);
    self
  }

  pub fn with_payload(mut self, payload: Payload) -> Self {
    self.payload = Some(toml::to_string(&payload).unwrap());
    self
  }

  pub fn with_vec<T: serde::Serialize>(mut self, payload: Vec<T>) -> Self {
    self.payload = Some(toml::to_string(&ArrayPayload { items: payload }).unwrap());
    self
  }

  pub fn as_string(self) -> String {
    toml::to_string(&self).unwrap()
  }

  pub fn parse_vec_payload<T: serde::de::DeserializeOwned>(&self) -> Option<Vec<T>> {
    self.parse_payload::<ArrayPayload<T>>().map(|x| x.items)
  }

  pub fn parse_payload<T: serde::de::DeserializeOwned>(&self) -> Option<T> {
    let Some(ref payload) = self.payload else {
      return None;
    };
    if let Ok(p) = serde_json::from_str(payload) {
      Some(p)
    } else if let Ok(p) = toml::from_str(payload) {
      Some(p)
    } else {
      None
    }
  }
}

impl From<MessageType> for Message {
  fn from(value: MessageType) -> Self {
    Self::from_type(value)
  }
}
