pub mod payloads;
#[cfg(feature = "server")]
pub mod recv;
pub mod send;
pub mod ser;

#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone)]
pub enum MessageType {
  Valid,
  Ok,
  Error,
  Unknown,
  RequestInput,
  #[default]
  Enquire,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone)]
pub struct Message {
  pub r#type: MessageType,
  pub action: String,
  pub payload: Option<String>,
  pub from_uid: Option<u32>,
  pub from_gid: Option<u32>,
  pub from_pid: Option<i32>,
}

impl Message {
  pub fn from_type(t: MessageType) -> Self {
    Self {
      r#type: t,
      payload: None,
      ..Default::default()
    }
  }

  pub fn from_action<T: AsRef<str>>(action: T) -> Self {
    Self::default().r#as(action)
  }

  pub fn with(mut self, payload: String) -> Self {
    self.payload = Some(payload);
    self
  }

  pub fn r#as<T: AsRef<str>>(mut self, action: T) -> Self {
    self.action = action.as_ref().to_string();
    self
  }

  pub fn ok(payload: impl Into<String>) -> Self {
    Self::from_type(MessageType::Ok).with(payload.into())
  }

  pub fn err(payload: impl Into<String>) -> Self {
    Self::from_type(MessageType::Error).with(payload.into())
  }

  pub fn with_vec<T: serde::Serialize>(mut self, payload: Vec<T>) -> Self {
    self.payload = serde_json::to_string(&payload).ok();
    self
  }

  pub fn as_string(self) -> String {
    serde_json::to_string(&self).unwrap_or_default()
  }

  pub fn parse_vec_payload<T: serde::de::DeserializeOwned>(&self) -> Option<Vec<T>> {
    self.parse_payload::<Vec<T>>().ok()
  }

  pub fn parse_payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
    let Some(ref payload) = self.payload else {
      return Err("Payload Not found".into());
    };

    if let Ok(p) = match serde_json::from_str::<T>(payload) {
      Err(e) => return Err(e.to_string()),
      Ok(e) => Ok::<T, String>(e),
    } {
      Ok(p)
    } else {
      Err("Nothing".into())
    }
  }

  pub fn from_uid(mut self, id: u32) -> Self {
    self.from_uid = Some(id);
    self
  }

  pub fn from_gid(mut self, id: u32) -> Self {
    self.from_gid = Some(id);
    self
  }

  pub fn from_pid(mut self, id: i32) -> Self {
    self.from_pid = Some(id);
    self
  }
}

impl From<MessageType> for Message {
  fn from(value: MessageType) -> Self {
    Self::from_type(value)
  }
}

impl From<&str> for Message {
  fn from(value: &str) -> Self {
    Self::from_action(value)
  }
}

impl Into<serde_json::Value> for Message {
  fn into(self) -> serde_json::Value {
    serde_json::to_value(self).unwrap()
  }
}

#[cfg(test)]
mod tests {
  use super::{Message, MessageType};

  #[test]
  fn message_roundtrip_serialization_contract() {
    let msg = Message::from_action("service.start")
      .with("{\"name\":\"units:demo\"}".to_string())
      .from_uid(1000)
      .from_gid(1000)
      .from_pid(4242);

    let raw = msg.clone().as_string();
    let decoded: Message = serde_json::from_str(&raw).expect("message should deserialize");
    assert_eq!(decoded.action, msg.action);
    assert_eq!(decoded.payload, msg.payload);
    assert_eq!(decoded.from_uid, Some(1000));
    assert_eq!(decoded.from_gid, Some(1000));
    assert_eq!(decoded.from_pid, Some(4242));
  }

  #[test]
  fn parse_payload_errors_when_missing_or_invalid() {
    let missing = Message::from_type(MessageType::Valid);
    assert!(missing.parse_payload::<u32>().is_err());

    let invalid = Message::from_action("x").with("not-json".to_string());
    assert!(invalid.parse_payload::<u32>().is_err());
  }

  #[test]
  fn with_vec_and_parse_vec_payload_roundtrip_property() {
    for len in 0..40usize {
      let values: Vec<u32> = (0..len as u32).map(|v| v * 3).collect();
      let msg = Message::from_action("list").with_vec(values.clone());
      let parsed = msg
        .parse_vec_payload::<u32>()
        .expect("payload should deserialize");
      assert_eq!(parsed, values);
    }
  }
}
