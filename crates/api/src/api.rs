use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::os::raw::c_char;
use std::os::unix::net::UnixStream;
use std::ptr::null_mut;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{ptr, thread};

use once_cell::sync::Lazy;
use rind_base::flow::{FlowJson, FlowPayload};
use rind_base::transport::TransportMessage;

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
pub struct MessageContainer {
  r#type: MessageType,
  action: MessageAction,
  payload: *mut PayloadContainer,
  name: *const c_char,
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
        MessageType::Signal => rind_base::transport::TransportMessageType::Signal,
        MessageType::State => rind_base::transport::TransportMessageType::State,
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
#[derive(Debug, PartialEq, Eq)]
pub enum MessageType {
  Signal = 0,
  State = 1,
  Enquiry = 2,
  Response = 3,
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

    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
      line.clear();
      let Ok(read) = reader.read_line(&mut line) else {
        break;
      };
      if read == 0 {
        break;
      }
      let trimmed = line.trim();

      let msg: MessageContainer = match serde_json::from_str::<TransportMessage>(&trimmed) {
        Ok(m) => MessageContainer {
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
            rind_base::transport::TransportMessageType::Signal => MessageType::Signal,
            rind_base::transport::TransportMessageType::State => MessageType::State,
          },
          action: match &m.action {
            rind_base::transport::TransportMessageAction::Remove => MessageAction::Remove,
            rind_base::transport::TransportMessageAction::Set => MessageAction::Set,
          },
          payload: if let Some(p) = m.payload {
            Box::into_raw(Box::new(PayloadContainer {
              content: CString::new(p.to_string_payload()).unwrap().as_ptr(),
              r#type: match p {
                FlowPayload::Json(_) => PayloadType::Json,
                _ => PayloadType::String,
              },
            }))
          } else {
            null_mut()
          },
        },
        Err(_) => continue,
      };

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
pub extern "C" fn send_message(tp: *const TransportProtocol, message: MessageContainer) {
  if tp.is_null() {
    return;
  }

  let tp = unsafe { &*tp };

  let msg: TransportMessage = { &message }.into();

  match tp.protocol {
    TransportProtocolMethod::STDIO => {
      println!("{}", serde_json::to_string(&msg).unwrap());
    }
    TransportProtocolMethod::UDS => {
      let mut stream = {
        let conns = UDS_CONNECTIONS.read().unwrap();

        let conn = conns.get(&tp.id).unwrap().try_clone().unwrap();
        drop(conns);
        conn
      };

      if stream
        .write_all(
          &serde_json::to_string(&msg)
            .map(|s| format!("{s}\n"))
            .unwrap_or_default()
            .into_bytes(),
        )
        .is_err()
      {
        return;
      }
    }
  }
}
