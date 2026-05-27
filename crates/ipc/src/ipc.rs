pub mod payloads;
#[cfg(feature = "server")]
pub mod recv;
pub mod send;
pub mod ser;
pub mod shm;

use std::{io::stdout, os::unix::net::UnixStream};

use rind_core::prelude::Ustr;
pub const IPC_MAGIC: [u8; 4] = *b"RIND";
pub const MAX_IPC_MESSAGE_SIZE: usize = 64 * 1024 * 1024; // 64MB

use serde::{Deserialize, Serialize};
use serde_json;

use crate::{
  ser::{deser_from_vec, ser_to_vec},
  shm::{ShmChannel, ShmStream},
};

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub enum MessageType {
  Valid,
  Ok,
  Error,
  Unknown,
  RequestInput,
  #[default]
  Enquire,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlowJson(pub String);

impl From<String> for FlowJson {
  fn from(value: String) -> Self {
    Self(value)
  }
}

impl FlowJson {
  pub fn into_json(&self) -> serde_json::Value {
    serde_json::from_str(&self.0).unwrap_or(serde_json::Value::Null)
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
  pub fn to_string_payload(&self) -> String {
    match self {
      FlowPayload::Json(v) => v.0.clone(),
      FlowPayload::String(v) => v.clone(),
      FlowPayload::Bytes(v) => String::from_utf8_lossy(v).to_string(),
      FlowPayload::None(_) => String::new(),
    }
  }

  pub fn from_json(v: Option<serde_json::Value>) -> Self {
    match v {
      Some(serde_json::Value::String(s)) => FlowPayload::String(s),
      Some(serde_json::Value::Object(v)) => {
        FlowPayload::Json(FlowJson(serde_json::Value::Object(v).to_string()))
      }
      Some(serde_json::Value::Array(v)) => {
        FlowPayload::Json(FlowJson(serde_json::Value::Array(v).to_string()))
      }
      Some(serde_json::Value::Null) | None => FlowPayload::None(false),
      Some(v) => FlowPayload::String(v.to_string()),
    }
  }

  pub fn to_json(&self) -> serde_json::Value {
    match self {
      FlowPayload::Json(v) => v.into_json(),
      FlowPayload::String(v) => serde_json::Value::String(v.clone()),
      FlowPayload::Bytes(v) => serde_json::Value::String(String::from_utf8_lossy(v).to_string()),
      FlowPayload::None(_) => serde_json::Value::Null,
    }
  }

  pub fn get_json_field(&self, field: &str) -> Option<serde_json::Value> {
    match self {
      FlowPayload::Json(s) => s.into_json().get(field).cloned(),
      _ => None,
    }
  }

  pub fn get_json_field_as<T: serde::de::DeserializeOwned>(&self, field: &str) -> Option<T> {
    match self {
      FlowPayload::Json(s) => serde_json::from_value(s.into_json().get(field).cloned()?).ok(),
      _ => None,
    }
  }

  pub fn contains(&self, needle: &str) -> bool {
    match self {
      FlowPayload::String(s) => s.contains(needle),
      FlowPayload::Json(s) => s.0.contains(needle),
      _ => false,
    }
  }

  pub fn set_json(&mut self, key: String, value: serde_json::Value) {
    match self {
      FlowPayload::Json(v) => {
        let mut json = v.into_json();
        if let Some(obj) = json.as_object_mut() {
          obj.insert(key, value);
          v.0 = json.to_string();
        }
      }
      _ => {}
    }
  }
}

#[derive(Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TransportMessageType {
  Impulse,
  Facet,
  Enquiry,
  Response,
  Unknown,
}

#[derive(Serialize, Deserialize, Default, PartialEq, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TransportMessageAction {
  #[default]
  Set,
  Remove,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowMatchOperation {
  Eq(Ustr),
  Options {
    binary: Option<bool>,
    contains: Option<Ustr>,
    r#as: Option<serde_json::Value>,
  },
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowPayloadType {
  #[default]
  Json,
  String,
  Bytes,
  None,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TransportMessage {
  pub r#type: TransportMessageType,
  pub payload: Option<FlowPayload>,
  pub branch: Option<FlowMatchOperation>,
  pub name: Option<Ustr>,
  #[serde(default)]
  pub action: TransportMessageAction,
}

impl TransportMessage {
  pub fn as_bytes(&self) -> Vec<u8> {
    ser_to_vec(self, true)
  }

  pub fn write_signed<W: std::io::Write>(&self, mut writer: W) -> std::io::Result<()> {
    let buf = self.as_bytes();
    writer.write_all(&crate::IPC_MAGIC)?;
    writer.write_all(&(buf.len() as u32).to_be_bytes())?;
    writer.write_all(&buf)?;
    Ok(())
  }

  pub fn read_signed<R: std::io::Read>(mut reader: R) -> std::io::Result<Self> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != crate::IPC_MAGIC {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Invalid IPC Magic",
      ));
    }
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > crate::MAX_IPC_MESSAGE_SIZE {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "IPC Message too large",
      ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    deser_from_vec(&buf, true)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
  }

  pub fn log<S: AsRef<str>>(message: S) -> Self {
    TransportMessage {
      action: TransportMessageAction::Set,
      branch: None,
      name: Some("log".into()),
      payload: Some(FlowPayload::String(message.as_ref().to_string())),
      r#type: TransportMessageType::Response,
    }
  }

  pub fn wlog<S: AsRef<str>>(message: S) {
    let _ = Self::log(message).write_signed(stdout());
  }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Default, Clone)]
pub struct Message {
  pub r#type: MessageType,
  pub action: String,
  #[serde(with = "serde_bytes")]
  pub payload: Option<Vec<u8>>,
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

  pub fn with_string(mut self, payload: String) -> Self {
    self.payload = Some(ser_to_vec(&payload, false));
    self
  }

  pub fn with(mut self, payload: Vec<u8>) -> Self {
    self.payload = Some(payload);
    self
  }

  pub fn with_slice<P: AsRef<[u8]>>(self, payload: P) -> Self {
    self.with(payload.as_ref().to_vec())
  }

  pub fn r#as<T: AsRef<str>>(mut self, action: T) -> Self {
    self.action = action.as_ref().to_string();
    self
  }

  pub fn ok(payload: impl Into<String>) -> Self {
    Self::from_type(MessageType::Ok).with_string(payload.into())
  }

  pub fn err(payload: impl Into<String>) -> Self {
    Self::from_type(MessageType::Error).with_string(payload.into())
  }

  pub fn with_vec<T: serde::Serialize>(mut self, payload: Vec<T>) -> Self {
    self.payload = Some(ser_to_vec(&payload, false));
    self
  }

  pub fn with_obj<T: serde::Serialize>(mut self, payload: T) -> Self {
    self.payload = Some(ser_to_vec(&payload, false));
    self
  }

  pub fn as_bytes(&self) -> Vec<u8> {
    ser_to_vec(self, false)
  }

  pub fn write_signed<W: std::io::Write>(&self, mut writer: W) -> std::io::Result<()> {
    let buf = self.as_bytes();
    writer.write_all(&crate::IPC_MAGIC)?;
    writer.write_all(&(buf.len() as u32).to_be_bytes())?;
    writer.write_all(&buf)?;
    Ok(())
  }

  pub fn read_signed<R: std::io::Read>(mut reader: R) -> std::io::Result<Self> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != crate::IPC_MAGIC {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Invalid IPC Magic",
      ));
    }
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > crate::MAX_IPC_MESSAGE_SIZE {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "IPC Message too large",
      ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    deser_from_vec(&buf, false)
      .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
  }

  pub fn parse_vec_payload<T: serde::de::DeserializeOwned>(&self) -> Option<Vec<T>> {
    self.parse_payload::<Vec<T>>().ok()
  }

  pub fn parse_payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
    let Some(ref payload) = self.payload else {
      return Err("Payload Not found".into());
    };

    if let Ok(p) = match deser_from_vec::<T>(payload, false) {
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

pub enum TransportStream {
  Uds(UnixStream),
  Shm(ShmStream),
  ShmChan(ShmChannel),
}

impl TransportStream {
  pub fn as_uds(self) -> Option<UnixStream> {
    match self {
      Self::Uds(stream) => Some(stream),
      _ => None,
    }
  }
  pub fn as_shm_chan(self) -> Option<ShmChannel> {
    match self {
      Self::ShmChan(stream) => Some(stream),
      _ => None,
    }
  }
  pub fn as_shm_chan_ref(&self) -> Option<&ShmChannel> {
    match self {
      Self::ShmChan(stream) => Some(stream),
      _ => None,
    }
  }
  pub fn as_shm(self) -> Option<ShmStream> {
    match self {
      Self::Shm(stream) => Some(stream),
      _ => None,
    }
  }
}

impl std::io::Read for TransportStream {
  fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    match self {
      Self::Shm(stream) => stream.read(buf),
      Self::Uds(stream) => stream.read(buf),
      Self::ShmChan(stream) => stream.read(buf),
    }
  }
}

impl std::io::Write for TransportStream {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    match self {
      Self::Shm(_) => Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("shm stream can not write"),
      )),
      Self::Uds(stream) => stream.write(buf),
      Self::ShmChan(stream) => stream.write(buf),
    }
  }

  fn flush(&mut self) -> std::io::Result<()> {
    match self {
      Self::Shm(_) => Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("shm stream can not flush"),
      )),
      Self::Uds(stream) => stream.flush(),
      Self::ShmChan(stream) => stream.flush(),
    }
  }
}

#[cfg(test)]
mod tests {
  use crate::ser::deser_from_vec;

  use super::{Message, MessageType};

  #[test]
  fn message_roundtrip_serialization_contract() {
    let msg = Message::from_action("service.start")
      .with_slice(b"{\"name\":\"units:demo\"}")
      .from_uid(1000)
      .from_gid(1000)
      .from_pid(4242);

    let raw = msg.clone().as_bytes();
    let decoded: Message = deser_from_vec(&raw, false).expect("message should deserialize");
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

    // let invalid = Message::from_action("x").with_slice("not-json".to_string());
    // assert!(invalid.parse_payload::<u32>().is_err());
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
