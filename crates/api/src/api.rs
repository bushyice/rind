// TODO: Add shm tp

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::stdout;
use std::os::raw::c_char;
use std::os::unix::net::UnixStream;
use std::ptr::null_mut;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{ptr, thread};

use once_cell::sync::Lazy;
use rind_base::flow::{FlowJson, FlowPayload};
use rind_base::transport::TransportMessage;
use rind_ipc::Message;
use rind_ipc::ser::flexbuf_string;

static UDS_CONNECTIONS: Lazy<RwLock<HashMap<u64, UnixStream>>> =
  Lazy::new(|| RwLock::new(HashMap::new()));

static TRANSPORT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum TransportProtocolMethod {
  STDIO = 0,
  UDS = 1,
}

#[repr(C)]
pub enum MessageAction {
  Remove = 0,
  Set = 1,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum MessageType {
  Impulse = 0,
  Facet = 1,
  Enquiry = 2,
  Response = 3,
  Unknown = 4,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum InvokeType {
  Valid = 0,
  Ok = 1,
  Error = 2,
  Unknown = 3,
  RequestInput = 4,
  Enquire = 5,
}

#[repr(C)]
pub struct MessageContainer {
  r#type: MessageType,
  action: MessageAction,
  payload: *mut PayloadContainer,
  name: *const c_char,
}

#[repr(C)]
pub struct InvokeCommand {
  r#type: InvokeType,
  action: *const c_char,
  payload: *const c_char,
}

impl Into<Message> for InvokeCommand {
  fn into(self) -> Message {
    Message {
      r#type: match self.r#type {
        InvokeType::Enquire => rind_ipc::MessageType::Enquire,
        InvokeType::Ok => rind_ipc::MessageType::Ok,
        InvokeType::Error => rind_ipc::MessageType::Error,
        InvokeType::Valid => rind_ipc::MessageType::Valid,
        InvokeType::RequestInput => rind_ipc::MessageType::RequestInput,
        InvokeType::Unknown => rind_ipc::MessageType::Unknown,
      },
      action: if !self.action.is_null() {
        unsafe { CStr::from_ptr(self.action) }
          .to_str()
          .unwrap()
          .to_string()
          .into()
      } else {
        "unknown".into()
      },
      payload: if !self.payload.is_null() {
        let s = unsafe { CStr::from_ptr(self.payload) }
          .to_str()
          .unwrap()
          .to_string();
        flexbuffers::to_vec(&s).ok()
      } else {
        None
      },
      from_uid: None,
      from_gid: None,
      from_pid: None,
    }
  }
}

impl Into<TransportMessage> for &MessageContainer {
  fn into(self) -> TransportMessage {
    TransportMessage {
      action: match self.action {
        MessageAction::Remove => rind_base::transport::TransportMessageAction::Remove,
        MessageAction::Set => rind_base::transport::TransportMessageAction::Set,
      },
      r#type: match self.r#type {
        MessageType::Enquiry => rind_base::transport::TransportMessageType::Enquiry,
        MessageType::Response => rind_base::transport::TransportMessageType::Response,
        MessageType::Impulse => rind_base::transport::TransportMessageType::Impulse,
        MessageType::Facet => rind_base::transport::TransportMessageType::Facet,
        MessageType::Unknown => rind_base::transport::TransportMessageType::Unknown,
      },
      branch: None,
      name: if self.name.is_null() {
        None
      } else {
        Some(
          unsafe { CStr::from_ptr(self.name) }
            .to_str()
            .unwrap()
            .to_string()
            .into(),
        )
      },
      payload: if self.payload.is_null() {
        None
      } else {
        Some(unsafe { &*self.payload }.into())
      },
    }
  }
}

#[repr(C)]
pub struct TransportProtocol {
  pub protocol: TransportProtocolMethod,
  pub options: *const *const c_char,
  pub len: usize,
  pub id: u64,
}

fn init_tp_internal(transport: TransportProtocol) -> TransportProtocol {
  if transport.protocol == TransportProtocolMethod::UDS {
    if transport.options.is_null() || transport.len == 0 {
      return transport;
    }

    let options: Vec<&str> = unsafe {
      let slice = std::slice::from_raw_parts(transport.options, transport.len);

      slice
        .iter()
        .map(|&ptr| {
          if ptr.is_null() {
            ""
          } else {
            CStr::from_ptr(ptr).to_str().unwrap()
          }
        })
        .collect()
    };

    match UnixStream::connect(options[0]) {
      Ok(stream) => {
        UDS_CONNECTIONS
          .write()
          .unwrap()
          .insert(transport.id, stream);
      }
      Err(e) => {
        eprintln!("{e}");
        return transport;
      }
    }
  }
  transport
}

#[unsafe(no_mangle)]
pub extern "C" fn init_tp(
  protocol: TransportProtocolMethod,
  options: *const c_char,
) -> TransportProtocol {
  if options.is_null() {
    return init_tp_internal(TransportProtocol {
      protocol,
      options: ptr::null(),
      len: 0,
      id: TRANSPORT_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
    });
  }

  let input = unsafe { CStr::from_ptr(options) }.to_str().unwrap();

  let parts: Vec<CString> = input.split(' ').map(|s| CString::new(s).unwrap()).collect();

  let ptrs: Vec<*const c_char> = parts.iter().map(|s| s.as_ptr()).collect();

  let boxed = ptrs.into_boxed_slice();
  let len = boxed.len();
  let ptr = boxed.as_ptr();

  std::mem::forget(boxed);
  std::mem::forget(parts);

  init_tp_internal(TransportProtocol {
    protocol,
    options: ptr,
    len,
    id: TRANSPORT_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
  })
}

fn transport_to_container(m: TransportMessage) -> MessageContainer {
  MessageContainer {
    name: match m.name {
      Some(s) => {
        let str = CString::new(&**s).unwrap();
        str.into_raw()
      }
      None => null_mut(),
    },
    r#type: match m.r#type {
      rind_base::transport::TransportMessageType::Enquiry => MessageType::Enquiry,
      rind_base::transport::TransportMessageType::Response => MessageType::Response,
      rind_base::transport::TransportMessageType::Impulse => MessageType::Impulse,
      rind_base::transport::TransportMessageType::Facet => MessageType::Facet,
      rind_base::transport::TransportMessageType::Unknown => MessageType::Unknown,
    },
    action: match &m.action {
      rind_base::transport::TransportMessageAction::Remove => MessageAction::Remove,
      rind_base::transport::TransportMessageAction::Set => MessageAction::Set,
    },
    payload: if let Some(p) = m.payload {
      Box::into_raw(Box::new(PayloadContainer {
        content: CString::new(p.to_string_payload()).unwrap().into_raw(),
        r#type: match p {
          FlowPayload::Json(_) => PayloadType::Json,
          _ => PayloadType::String,
        },
      }))
    } else {
      null_mut()
    },
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn listen_tp(
  tp: *mut TransportProtocol,
  func: unsafe extern "C" fn(MessageContainer),
) {
  if tp.is_null() {
    return;
  }

  let tp = unsafe { &*tp };
  let id = tp.id;

  // let func: unsafe extern "C" fn(MessageContainer) -> *const MessageContainer = unsafe { *func };

  thread::spawn(move || {
    let stream = {
      let conns = UDS_CONNECTIONS.read().unwrap();

      let conn = conns.get(&id).unwrap().try_clone().unwrap();
      drop(conns);
      conn
    };

    while let Ok(m) = TransportMessage::read_signed(&stream) {
      let msg = transport_to_container(m);
      let _ = unsafe { func(msg) };

      // if !response.is_null() {
      //   let response = unsafe { &*response };
      //   let msg: TransportMessage = response.into();
      //   let resp_str = serde_json::to_string(&msg).unwrap_or_default().into_bytes();
      //   let resp_len = (resp_str.len() as u32).to_be_bytes();

      //   if stream.write_all(&resp_len).is_err() {
      //     break;
      //   }
      //   if stream.write_all(&resp_str).is_err() {
      //     break;
      //   }
      // }
    }
  });
}

#[unsafe(no_mangle)]
pub extern "C" fn enquiry_tp(
  tp: *const TransportProtocol,
  message: MessageContainer,
) -> MessageContainer {
  if tp.is_null() {
    return MessageContainer {
      r#type: MessageType::Unknown,
      action: MessageAction::Remove,
      payload: null_mut(),
      name: null_mut(),
    };
  }

  let tp = unsafe { &*tp };

  let mut stream = match tp.protocol {
    TransportProtocolMethod::STDIO => {
      return MessageContainer {
        r#type: MessageType::Unknown,
        action: MessageAction::Remove,
        payload: null_mut(),
        name: null_mut(),
      };
    }
    TransportProtocolMethod::UDS => {
      let options: Vec<&str> = if tp.options.is_null() || tp.len == 0 {
        Vec::new()
      } else {
        unsafe {
          let slice = std::slice::from_raw_parts(tp.options, tp.len);
          slice
            .iter()
            .map(|&ptr| {
              if ptr.is_null() {
                ""
              } else {
                CStr::from_ptr(ptr).to_str().unwrap()
              }
            })
            .collect()
        }
      };

      if options.is_empty() {
        return MessageContainer {
          r#type: MessageType::Unknown,
          action: MessageAction::Remove,
          payload: null_mut(),
          name: null_mut(),
        };
      }

      match UnixStream::connect(options[0]) {
        Ok(s) => s,
        Err(_) => {
          return MessageContainer {
            r#type: MessageType::Unknown,
            action: MessageAction::Remove,
            payload: null_mut(),
            name: null_mut(),
          };
        }
      }
    }
  };

  let msg: TransportMessage = { &message }.into();
  if msg.write_signed(&mut stream).is_err() {
    return MessageContainer {
      r#type: MessageType::Unknown,
      action: MessageAction::Remove,
      payload: null_mut(),
      name: null_mut(),
    };
  }

  match TransportMessage::read_signed(&mut stream) {
    Ok(m) => transport_to_container(m),
    Err(_) => MessageContainer {
      r#type: MessageType::Unknown,
      action: MessageAction::Remove,
      payload: null_mut(),
      name: null_mut(),
    },
  }
}

#[repr(C)]
pub struct PayloadContainer {
  r#type: PayloadType,
  content: *const c_char,
}

impl Into<FlowPayload> for &PayloadContainer {
  fn into(self) -> FlowPayload {
    let inner = unsafe { CStr::from_ptr(self.content) }
      .to_str()
      .unwrap()
      .to_string();

    if matches!(self.r#type, PayloadType::Json) {
      FlowPayload::Json(FlowJson::from(inner))
    } else {
      FlowPayload::String(inner)
    }
  }
}

#[repr(C)]
pub enum PayloadType {
  String = 0,
  Json = 1,
}

#[unsafe(no_mangle)]
pub extern "C" fn create_message(r#type: MessageType, action: MessageAction) -> MessageContainer {
  MessageContainer {
    r#type,
    action,
    payload: null_mut(),
    name: null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn create_message_payload(
  r#type: PayloadType,
  inner: *const c_char,
) -> PayloadContainer {
  PayloadContainer {
    r#type,
    content: inner,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn set_message_payload(message: *mut MessageContainer, payload: PayloadContainer) {
  let msg = unsafe { &mut *message };
  msg.payload = Box::into_raw(Box::new(payload));
}

#[unsafe(no_mangle)]
pub extern "C" fn set_message_name(message: *mut MessageContainer, name: *const c_char) {
  let msg = unsafe { &mut *message };
  msg.name = name;
}

#[unsafe(no_mangle)]
pub extern "C" fn set_state(name: *const c_char, payload: PayloadContainer) -> MessageContainer {
  let mut msg = create_message(MessageType::Facet, MessageAction::Set);
  msg.name = name;
  msg.payload = Box::into_raw(Box::new(payload));
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn remove_state(
  name: *const c_char,
  payload: *mut PayloadContainer,
) -> MessageContainer {
  let mut msg = create_message(MessageType::Facet, MessageAction::Remove);
  msg.name = name;
  if !payload.is_null() {
    msg.payload = payload;
  }
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn emit_signal(
  name: *const c_char,
  payload: *mut PayloadContainer,
) -> MessageContainer {
  let mut msg = create_message(MessageType::Impulse, MessageAction::Set);
  msg.name = name;
  if !payload.is_null() {
    msg.payload = payload;
  }
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn send_message(tp: *const TransportProtocol, message: MessageContainer) {
  if tp.is_null() {
    return;
  }

  let tp = unsafe { &*tp };

  let msg: TransportMessage = { &message }.into();

  match tp.protocol {
    TransportProtocolMethod::STDIO => {
      // println!("{}", serde_json::to_string(&msg).unwrap());
      let _ = msg.write_signed(stdout());
    }
    TransportProtocolMethod::UDS => {
      let stream = {
        let conns = UDS_CONNECTIONS.read().unwrap();

        let conn = conns.get(&tp.id).unwrap().try_clone().unwrap();
        drop(conns);
        conn
      };

      let _ = msg.write_signed(stream);
    }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn create_invoke(
  r#type: InvokeType,
  action: *const c_char,
  payload: *const c_char,
) -> InvokeCommand {
  InvokeCommand {
    r#type,
    action,
    payload,
  }
}

static RIND_SOCK_PATH: Lazy<RwLock<String>> = Lazy::new(|| RwLock::new("/tmp/rind.sock".into()));

#[unsafe(no_mangle)]
pub extern "C" fn set_rind_sock_path(path: *mut c_char) {
  *RIND_SOCK_PATH.write().unwrap() = unsafe { CString::from_raw(path) }
    .to_string_lossy()
    .to_string();
}

#[unsafe(no_mangle)]
pub extern "C" fn invoke(command: InvokeCommand) -> InvokeCommand {
  let Ok(mut stream) = UnixStream::connect(RIND_SOCK_PATH.read().unwrap().clone()) else {
    return InvokeCommand {
      action: null_mut(),
      payload: null_mut(),
      r#type: InvokeType::Error,
    };
  };

  let msg: Message = command.into();

  let Ok(_) = msg.write_signed(&stream) else {
    return InvokeCommand {
      r#type: InvokeType::Error,
      action: null_mut(),
      payload: null_mut(),
    };
  };

  let Ok(msg) = Message::read_signed(&mut stream) else {
    return InvokeCommand {
      r#type: InvokeType::Error,
      action: null_mut(),
      payload: null_mut(),
    };
  };

  InvokeCommand {
    action: {
      let str = CString::new(msg.action).unwrap();
      str.into_raw()
    },
    payload: if let Some(p) = msg.payload {
      let str = CString::new(flexbuf_string(p)).unwrap();
      str.into_raw()
    } else {
      null_mut()
    },
    r#type: match msg.r#type {
      rind_ipc::MessageType::Enquire => InvokeType::Enquire,
      rind_ipc::MessageType::Ok => InvokeType::Ok,
      rind_ipc::MessageType::Error => InvokeType::Error,
      rind_ipc::MessageType::Valid => InvokeType::Valid,
      rind_ipc::MessageType::RequestInput => InvokeType::RequestInput,
      rind_ipc::MessageType::Unknown => InvokeType::Unknown,
    },
  }
}
