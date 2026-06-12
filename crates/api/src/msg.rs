use rind_ipc::ser::{deser_string, ser_to_vec};
use rind_ipc::{
  FlowJson, FlowPayload, Message as IpcMessage, MessageType as IpcMessageType, TransportMessage,
  TransportMessageAction, TransportMessageType,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageAction {
  Set,
  Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
  Impulse,
  Facet,
  Enquiry,
  Response,
  Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadType {
  String,
  Json,
}

#[derive(Debug, Clone)]
pub struct Payload {
  pub r#type: PayloadType,
  pub content: String,
}

impl Payload {
  pub fn string(content: impl Into<String>) -> Self {
    Payload {
      r#type: PayloadType::String,
      content: content.into(),
    }
  }

  pub fn json(content: impl Into<String>) -> Self {
    Payload {
      r#type: PayloadType::Json,
      content: content.into(),
    }
  }

  pub fn json_value(value: serde_json::Value) -> Self {
    Payload {
      r#type: PayloadType::Json,
      content: value.to_string(),
    }
  }

  fn to_flow(&self) -> FlowPayload {
    match self.r#type {
      PayloadType::Json => FlowPayload::Json(FlowJson::from(self.content.clone())),
      PayloadType::String => FlowPayload::String(self.content.clone()),
    }
  }
}

#[derive(Debug, Clone)]
pub struct Message {
  pub r#type: MessageType,
  pub action: MessageAction,
  pub payload: Option<Payload>,
  pub name: Option<String>,
}

impl Message {
  pub fn enquiry(name: impl Into<String>, payload: Option<Payload>) -> Self {
    Message {
      r#type: MessageType::Enquiry,
      action: MessageAction::Set,
      payload,
      name: Some(name.into()),
    }
  }

  pub fn set_facet(name: impl Into<String>, payload: Payload) -> Self {
    Message {
      r#type: MessageType::Facet,
      action: MessageAction::Set,
      payload: Some(payload),
      name: Some(name.into()),
    }
  }

  pub fn remove_facet(name: impl Into<String>, payload: Option<Payload>) -> Self {
    Message {
      r#type: MessageType::Facet,
      action: MessageAction::Remove,
      payload,
      name: Some(name.into()),
    }
  }

  pub fn impulse(name: impl Into<String>, payload: Option<Payload>) -> Self {
    Message {
      r#type: MessageType::Impulse,
      action: MessageAction::Set,
      payload,
      name: Some(name.into()),
    }
  }

  pub fn log(log: impl Into<String>) -> Self {
    Message {
      r#type: MessageType::Response,
      action: MessageAction::Set,
      payload: Some(Payload::string(log.into())),
      name: Some("log".into()),
    }
  }

  pub(crate) fn to_transport(&self) -> TransportMessage {
    TransportMessage {
      action: match self.action {
        MessageAction::Remove => TransportMessageAction::Remove,
        MessageAction::Set => TransportMessageAction::Set,
      },
      r#type: match self.r#type {
        MessageType::Enquiry => TransportMessageType::Enquiry,
        MessageType::Response => TransportMessageType::Response,
        MessageType::Impulse => TransportMessageType::Impulse,
        MessageType::Facet => TransportMessageType::Facet,
        MessageType::Unknown => TransportMessageType::Unknown,
      },
      branch: None,
      name: self.name.as_ref().map(|s| s.as_str().into()),
      payload: self.payload.as_ref().map(|p| p.to_flow()),
    }
  }

  pub(crate) fn from_transport(m: TransportMessage) -> Self {
    Message {
      r#type: match m.r#type {
        TransportMessageType::Enquiry => MessageType::Enquiry,
        TransportMessageType::Response => MessageType::Response,
        TransportMessageType::Impulse => MessageType::Impulse,
        TransportMessageType::Facet => MessageType::Facet,
        TransportMessageType::Unknown => MessageType::Unknown,
      },
      action: match m.action {
        TransportMessageAction::Remove => MessageAction::Remove,
        TransportMessageAction::Set => MessageAction::Set,
      },
      payload: m.payload.map(|p| Payload {
        r#type: match &p {
          FlowPayload::Json(_) => PayloadType::Json,
          _ => PayloadType::String,
        },
        content: p.to_string_payload(),
      }),
      name: m.name.map(|s| s.to_string()),
    }
  }

  pub fn write_signed<W: std::io::Write>(&self, writer: W) -> std::io::Result<()> {
    self.to_transport().write_signed(writer)
  }

  pub fn read_signed<R: std::io::Read>(reader: R) -> std::io::Result<Self> {
    TransportMessage::read_signed(reader).map(Self::from_transport)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeType {
  Valid,
  Ok,
  Error,
  Unknown,
  RequestInput,
  Enquire,
}

#[derive(Debug, Clone)]
pub struct InvokeCommand {
  pub r#type: InvokeType,
  pub action: Option<String>,
  pub payload: Option<String>,
}

impl InvokeCommand {
  pub(crate) fn to_message(&self) -> IpcMessage {
    IpcMessage {
      r#type: match self.r#type {
        InvokeType::Enquire => IpcMessageType::Enquire,
        InvokeType::Ok => IpcMessageType::Ok,
        InvokeType::Error => IpcMessageType::Error,
        InvokeType::Valid => IpcMessageType::Valid,
        InvokeType::RequestInput => IpcMessageType::RequestInput,
        InvokeType::Unknown => IpcMessageType::Unknown,
      },
      action: self.action.clone().unwrap_or_else(|| "unknown".into()),
      payload: self.payload.as_ref().map(|s| ser_to_vec(s, false)),
      from_uid: None,
      from_gid: None,
      from_pid: None,
    }
  }

  pub(crate) fn from_message(msg: IpcMessage) -> Self {
    InvokeCommand {
      r#type: match msg.r#type {
        IpcMessageType::Enquire => InvokeType::Enquire,
        IpcMessageType::Ok => InvokeType::Ok,
        IpcMessageType::Error => InvokeType::Error,
        IpcMessageType::Valid => InvokeType::Valid,
        IpcMessageType::RequestInput => InvokeType::RequestInput,
        IpcMessageType::Unknown => InvokeType::Unknown,
      },
      action: Some(msg.action),
      payload: msg.payload.map(deser_string),
    }
  }
}
