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
  Ack,
  Nack,
  Error,
  Unknown,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Message {
  pub r#type: MessageType,
  pub payload: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct MessagePayload {
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

  pub fn ack(payload: impl Into<String>) -> Self {
    Self::from_type(MessageType::Ack).with(payload.into())
  }

  pub fn nack(payload: impl Into<String>) -> Self {
    Self::from_type(MessageType::Nack).with(payload.into())
  }

  pub fn with_payload(mut self, payload: MessagePayload) -> Self {
    self.payload = toml::to_string(&payload).ok();
    self
  }

  pub fn with_vec<T: serde::Serialize>(mut self, payload: Vec<T>) -> Self {
    self.payload = toml::to_string(&ArrayPayload { items: payload }).ok();
    self
  }

  pub fn as_string(self) -> String {
    toml::to_string(&self).unwrap_or_default()
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

#[cfg(test)]
mod tests {
  use super::{Message, MessagePayload, MessageType};

  #[test]
  fn payload_roundtrip_toml() {
    let msg = Message::from_type(MessageType::List).with_payload(MessagePayload {
      name: "unit@svc".to_string(),
      unit_type: rind_common::UnitType::Service,
      force: Some(true),
    });
    let parsed = msg.parse_payload::<MessagePayload>();
    assert!(parsed.is_some());
    let payload = parsed.unwrap_or(MessagePayload {
      name: String::new(),
      unit_type: rind_common::UnitType::Unknown,
      force: None,
    });
    assert_eq!(payload.name, "unit@svc".to_string());
    assert_eq!(payload.force, Some(true));
  }

  #[test]
  fn parse_payload_accepts_json_and_rejects_invalid() {
    let json_payload = serde_json::to_string(&MessagePayload {
      name: "x".to_string(),
      unit_type: rind_common::UnitType::Service,
      force: Some(false),
    })
    .unwrap_or_default();
    let json_msg = Message {
      r#type: MessageType::List,
      payload: Some(json_payload),
    };
    assert!(json_msg.parse_payload::<MessagePayload>().is_some());

    let invalid = Message {
      r#type: MessageType::List,
      payload: Some("not-json-not-toml".to_string()),
    };
    assert!(invalid.parse_payload::<MessagePayload>().is_none());
  }

  #[test]
  fn ack_and_nack_helpers_set_types() {
    let ack = Message::ack("ok");
    assert!(matches!(ack.r#type, MessageType::Ack));
    assert_eq!(ack.payload.unwrap_or_default(), "ok".to_string());

    let nack = Message::nack("bad");
    assert!(matches!(nack.r#type, MessageType::Nack));
    assert_eq!(nack.payload.unwrap_or_default(), "bad".to_string());
  }
}
