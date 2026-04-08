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
