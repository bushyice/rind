// TODO: Add shm tp

#![allow(non_camel_case_types)]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{Write, stdin, stdout};
use std::os::raw::c_char;
use std::os::unix::net::UnixStream;
use std::ptr::null_mut;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{ptr, thread};

use once_cell::sync::Lazy;
use rind_flow::transport::TransportMessage;
use rind_flow::{FlowJson, FlowPayload};
use rind_ipc::ser::{deser_from_vec, deser_string, ser_to_vec};
use rind_ipc::shm::{ShmChannel, shm_client_connect};
use rind_ipc::{Message, TransportMessageAction, TransportMessageType, TransportStream};

const SHM_SIZE: usize = 1024 * 1024;

static UDS_CONNECTIONS: Lazy<RwLock<HashMap<u64, UnixStream>>> =
  Lazy::new(|| RwLock::new(HashMap::new()));
static TP_STREAMS: Lazy<RwLock<HashMap<u64, TransportStream>>> =
  Lazy::new(|| RwLock::new(HashMap::new()));
static SHM_CONNECTIONS: Lazy<RwLock<HashMap<u64, ShmChannel>>> =
  Lazy::new(|| RwLock::new(HashMap::new()));

static TRANSPORT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RIND_TP_METHOD {
  STDIO = 0,
  UDS = 1,
  SHM = 2,
}

#[repr(C)]
pub enum RIND_MSG_ACTION {
  REMOVE = 0,
  SET = 1,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum RIND_MSG_TYPE {
  IMPULSE = 0,
  FACET = 1,
  ENQUIRY = 2,
  RESPONSE = 3,
  UNKNOWN = 4,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq)]
pub enum RIND_INVOKE_TYPE {
  VALID = 0,
  OK = 1,
  ERROR = 2,
  UNKNOWN = 3,
  REQUEST_INPUT = 4,
  ENQUIRE = 5,
}

#[repr(C)]
pub struct rind_msg {
  r#type: RIND_MSG_TYPE,
  action: RIND_MSG_ACTION,
  payload: *mut rind_payload,
  name: *const c_char,
}

#[repr(C)]
pub struct rind_invoke_cmd {
  r#type: RIND_INVOKE_TYPE,
  action: *const c_char,
  payload: *const c_char,
}

impl Into<Message> for rind_invoke_cmd {
  fn into(self) -> Message {
    Message {
      r#type: match self.r#type {
        RIND_INVOKE_TYPE::ENQUIRE => rind_ipc::MessageType::Enquire,
        RIND_INVOKE_TYPE::OK => rind_ipc::MessageType::Ok,
        RIND_INVOKE_TYPE::ERROR => rind_ipc::MessageType::Error,
        RIND_INVOKE_TYPE::VALID => rind_ipc::MessageType::Valid,
        RIND_INVOKE_TYPE::REQUEST_INPUT => rind_ipc::MessageType::RequestInput,
        RIND_INVOKE_TYPE::UNKNOWN => rind_ipc::MessageType::Unknown,
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
        Some(ser_to_vec(&s, false))
      } else {
        None
      },
      from_uid: None,
      from_gid: None,
      from_pid: None,
    }
  }
}

impl Into<TransportMessage> for &rind_msg {
  fn into(self) -> TransportMessage {
    TransportMessage {
      action: match self.action {
        RIND_MSG_ACTION::REMOVE => TransportMessageAction::Remove,
        RIND_MSG_ACTION::SET => TransportMessageAction::Set,
      },
      r#type: match self.r#type {
        RIND_MSG_TYPE::ENQUIRY => TransportMessageType::Enquiry,
        RIND_MSG_TYPE::RESPONSE => TransportMessageType::Response,
        RIND_MSG_TYPE::IMPULSE => TransportMessageType::Impulse,
        RIND_MSG_TYPE::FACET => TransportMessageType::Facet,
        RIND_MSG_TYPE::UNKNOWN => TransportMessageType::Unknown,
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
pub struct rind_tp {
  pub protocol: RIND_TP_METHOD,
  pub options: *const *const c_char,
  pub len: usize,
  pub id: u64,
}

fn init_tp_internal(transport: rind_tp) -> rind_tp {
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

  let mut retries = 0;

  let connect_tp = || {
    if transport.protocol == RIND_TP_METHOD::UDS {
      match UnixStream::connect(options[0]) {
        Ok(stream) => {
          UDS_CONNECTIONS
            .write()
            .unwrap()
            .insert(transport.id, stream);
        }
        Err(e) => {
          return Err(e.to_string());
        }
      }
    } else if transport.protocol == RIND_TP_METHOD::SHM {
      match shm_client_connect(SHM_SIZE, options[0]) {
        Ok(stream) => {
          SHM_CONNECTIONS
            .write()
            .unwrap()
            .insert(transport.id, stream);
        }
        Err(e) => {
          return Err(e.to_string());
        }
      }
    }

    Ok::<(), String>(())
  };

  let mut last = Ok::<(), String>(());

  while retries < 5 {
    let res = connect_tp();
    match &res {
      Ok(_) => break,
      Err(e) => {
        last = Err(format!("Failed to create connection: {e}"));
        if !e.contains("no such file") {
          eprintln!("{e}");
          break;
        }
      }
    }
    retries += 1;
    std::thread::sleep(std::time::Duration::from_millis(100));
  }

  last.unwrap();

  transport
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_init_tp(protocol: RIND_TP_METHOD, options: *const c_char) -> rind_tp {
  if options.is_null() {
    return init_tp_internal(rind_tp {
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

  init_tp_internal(rind_tp {
    protocol,
    options: ptr,
    len,
    id: TRANSPORT_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
  })
}

fn transport_to_container(m: TransportMessage) -> rind_msg {
  rind_msg {
    name: match m.name {
      Some(s) => {
        let str = CString::new(&**s).unwrap();
        str.into_raw()
      }
      None => null_mut(),
    },
    r#type: match m.r#type {
      TransportMessageType::Enquiry => RIND_MSG_TYPE::ENQUIRY,
      TransportMessageType::Response => RIND_MSG_TYPE::RESPONSE,
      TransportMessageType::Impulse => RIND_MSG_TYPE::IMPULSE,
      TransportMessageType::Facet => RIND_MSG_TYPE::FACET,
      TransportMessageType::Unknown => RIND_MSG_TYPE::UNKNOWN,
    },
    action: match &m.action {
      TransportMessageAction::Remove => RIND_MSG_ACTION::REMOVE,
      TransportMessageAction::Set => RIND_MSG_ACTION::SET,
    },
    payload: if let Some(p) = m.payload {
      Box::into_raw(Box::new(rind_payload {
        content: CString::new(p.to_string_payload()).unwrap().into_raw(),
        r#type: match p {
          FlowPayload::Json(_) => RIND_PAYLOAD_TYPE::JSON,
          _ => RIND_PAYLOAD_TYPE::STRING,
        },
      }))
    } else {
      null_mut()
    },
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_listen_tp(tp: *mut rind_tp, func: unsafe extern "C" fn(rind_msg)) {
  if tp.is_null() {
    return;
  }

  let tp = unsafe { &*tp };
  let id = tp.id;

  // let func: unsafe extern "C" fn(MessageContainer) -> *const MessageContainer = unsafe { *func };

  let protocol = tp.protocol;
  thread::spawn(move || match protocol {
    RIND_TP_METHOD::STDIO => {
      while let Ok(m) = TransportMessage::read_signed(&stdin()) {
        let msg = transport_to_container(m);
        let _ = unsafe { func(msg) };
      }
    }
    RIND_TP_METHOD::UDS | RIND_TP_METHOD::SHM => {
      let mut stream: TransportStream = if protocol == RIND_TP_METHOD::SHM {
        let mut conns = SHM_CONNECTIONS.write().unwrap();

        let Some(conn) = conns.get_mut(&id).unwrap().take_ingress() else {
          return;
        };
        drop(conns);
        TransportStream::Shm(conn)
      } else {
        let conns = UDS_CONNECTIONS.read().unwrap();

        let conn = conns.get(&id).unwrap().try_clone().unwrap();
        drop(conns);
        TransportStream::Uds(conn)
      };

      if protocol == RIND_TP_METHOD::UDS {
        while let Ok(m) = TransportMessage::read_signed(&mut stream) {
          let msg = transport_to_container(m);
          let _ = unsafe { func(msg) };
        }
      } else {
        let stream = stream.as_shm().unwrap();
        loop {
          match stream.evt.read() {
            Ok(_) => {
              while let Some(data) = stream.ring.read() {
                if let Ok(msg) = deser_from_vec::<TransportMessage>(&data, true) {
                  let msg = transport_to_container(msg);
                  let _ = unsafe { func(msg) };
                }
              }
            }
            Err(e) => {
              eprintln!("EventFd read error: {e}");
              break;
            }
          }
        }
      }
    }
  });
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_enquiry_tp(tp: *const rind_tp, message: rind_msg) -> rind_msg {
  if tp.is_null() {
    return rind_msg {
      r#type: RIND_MSG_TYPE::UNKNOWN,
      action: RIND_MSG_ACTION::REMOVE,
      payload: null_mut(),
      name: null_mut(),
    };
  }

  let tp = unsafe { &*tp };

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
    return rind_msg {
      r#type: RIND_MSG_TYPE::UNKNOWN,
      action: RIND_MSG_ACTION::REMOVE,
      payload: null_mut(),
      name: null_mut(),
    };
  }

  let mut stream: TransportStream =
    match tp.protocol {
      RIND_TP_METHOD::STDIO => {
        return rind_msg {
          r#type: RIND_MSG_TYPE::UNKNOWN,
          action: RIND_MSG_ACTION::REMOVE,
          payload: null_mut(),
          name: null_mut(),
        };
      }
      RIND_TP_METHOD::SHM => TP_STREAMS.write().unwrap().remove(&tp.id).unwrap_or(
        match shm_client_connect(SHM_SIZE, options[0]) {
          Ok(s) => TransportStream::ShmChan(s),
          Err(_) => {
            return rind_msg {
              r#type: RIND_MSG_TYPE::UNKNOWN,
              action: RIND_MSG_ACTION::REMOVE,
              payload: null_mut(),
              name: null_mut(),
            };
          }
        },
      ),
      RIND_TP_METHOD::UDS => TP_STREAMS.write().unwrap().remove(&tp.id).unwrap_or(
        match UnixStream::connect(options[0]) {
          Ok(s) => TransportStream::Uds(s),
          Err(_) => {
            return rind_msg {
              r#type: RIND_MSG_TYPE::UNKNOWN,
              action: RIND_MSG_ACTION::REMOVE,
              payload: null_mut(),
              name: null_mut(),
            };
          }
        },
      ),
    };

  let msg: TransportMessage = { &message }.into();
  if msg.write_signed(&mut stream).is_err() {
    return rind_msg {
      r#type: RIND_MSG_TYPE::UNKNOWN,
      action: RIND_MSG_ACTION::REMOVE,
      payload: null_mut(),
      name: null_mut(),
    };
  }

  match if tp.protocol == RIND_TP_METHOD::SHM {
    let stream = stream.as_shm_chan_ref().unwrap();
    let ingress = stream.ingress.as_ref().unwrap();
    match ingress.evt.read() {
      Ok(_) => {
        let mut data = Vec::new();
        while let Some(d) = ingress.ring.read() {
          data = d;
        }
        deser_from_vec::<TransportMessage>(&data, true)
          .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))
      }
      Err(e) => Err::<TransportMessage, std::io::Error>(std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        format!("{e}"),
      )),
    }
  } else {
    TransportMessage::read_signed(&mut stream)
  } {
    Ok(m) => {
      TP_STREAMS.write().unwrap().insert(tp.id, stream);
      transport_to_container(m)
    }
    Err(_) => rind_msg {
      r#type: RIND_MSG_TYPE::UNKNOWN,
      action: RIND_MSG_ACTION::REMOVE,
      payload: null_mut(),
      name: null_mut(),
    },
  }
}

#[repr(C)]
pub struct rind_payload {
  r#type: RIND_PAYLOAD_TYPE,
  content: *const c_char,
}

impl Into<FlowPayload> for &rind_payload {
  fn into(self) -> FlowPayload {
    let inner = unsafe { CStr::from_ptr(self.content) }
      .to_str()
      .unwrap()
      .to_string();

    if matches!(self.r#type, RIND_PAYLOAD_TYPE::JSON) {
      FlowPayload::Json(FlowJson::from(inner))
    } else {
      FlowPayload::String(inner)
    }
  }
}

#[repr(C)]
pub enum RIND_PAYLOAD_TYPE {
  STRING = 0,
  JSON = 1,
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_create_msg(r#type: RIND_MSG_TYPE, action: RIND_MSG_ACTION) -> rind_msg {
  rind_msg {
    r#type,
    action,
    payload: null_mut(),
    name: null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_create_msg_payload(
  r#type: RIND_PAYLOAD_TYPE,
  inner: *const c_char,
) -> rind_payload {
  rind_payload {
    r#type,
    content: inner,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_message_payload(message: *mut rind_msg, payload: rind_payload) {
  let msg = unsafe { &mut *message };
  msg.payload = Box::into_raw(Box::new(payload));
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_message_name(message: *mut rind_msg, name: *const c_char) {
  let msg = unsafe { &mut *message };
  msg.name = name;
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_msg_enquire(name: *const c_char, payload: *const c_char) -> rind_msg {
  let mut msg = rind_create_msg(RIND_MSG_TYPE::ENQUIRY, RIND_MSG_ACTION::SET);
  msg.name = name;
  if !payload.is_null() {
    msg.payload = Box::into_raw(Box::new(rind_create_msg_payload(
      RIND_PAYLOAD_TYPE::STRING,
      payload,
    )));
  }
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_facet(name: *const c_char, payload: rind_payload) -> rind_msg {
  let mut msg = rind_create_msg(RIND_MSG_TYPE::FACET, RIND_MSG_ACTION::SET);
  msg.name = name;
  msg.payload = Box::into_raw(Box::new(payload));
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_remove_facet(name: *const c_char, payload: *mut rind_payload) -> rind_msg {
  let mut msg = rind_create_msg(RIND_MSG_TYPE::FACET, RIND_MSG_ACTION::REMOVE);
  msg.name = name;
  if !payload.is_null() {
    msg.payload = payload;
  }
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_impulse(name: *const c_char, payload: *mut rind_payload) -> rind_msg {
  let mut msg = rind_create_msg(RIND_MSG_TYPE::IMPULSE, RIND_MSG_ACTION::SET);
  msg.name = name;
  if !payload.is_null() {
    msg.payload = payload;
  }
  msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_log_msg(log: *const c_char) -> rind_msg {
  let msg = TransportMessage::log(unsafe { CStr::from_ptr(log) }.to_str().unwrap().to_string());
  transport_to_container(msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_send_message(tp: *const rind_tp, message: rind_msg) -> u8 {
  if tp.is_null() {
    return 1;
  }

  let tp = unsafe { &*tp };

  let msg: TransportMessage = { &message }.into();

  match tp.protocol {
    RIND_TP_METHOD::STDIO => {
      let _ = msg.write_signed(stdout());
    }
    RIND_TP_METHOD::SHM => {
      let mut conns = SHM_CONNECTIONS.write().unwrap();
      let conn = conns.get_mut(&tp.id).unwrap();

      let data = msg.as_bytes();
      let _ = conn.write(&data);
    }
    RIND_TP_METHOD::UDS => {
      let stream = {
        let conns = UDS_CONNECTIONS.read().unwrap();

        let conn = conns.get(&tp.id).unwrap().try_clone().unwrap();
        drop(conns);
        conn
      };

      let _ = msg.write_signed(stream);
    }
  }

  return 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_create_invoke(
  r#type: RIND_INVOKE_TYPE,
  action: *const c_char,
  payload: *const c_char,
) -> rind_invoke_cmd {
  rind_invoke_cmd {
    r#type,
    action,
    payload,
  }
}

static RIND_SOCK_PATH: Lazy<RwLock<String>> = Lazy::new(|| RwLock::new("/tmp/rind.sock".into()));

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_sock_path(path: *mut c_char) {
  *RIND_SOCK_PATH.write().unwrap() = unsafe { CStr::from_ptr(path) }
    .to_string_lossy()
    .to_string();
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_invoke(command: rind_invoke_cmd) -> rind_invoke_cmd {
  let Ok(mut stream) = UnixStream::connect(RIND_SOCK_PATH.read().unwrap().clone()) else {
    return rind_invoke_cmd {
      action: null_mut(),
      payload: null_mut(),
      r#type: RIND_INVOKE_TYPE::ERROR,
    };
  };

  let msg: Message = command.into();

  let Ok(_) = msg.write_signed(&stream) else {
    return rind_invoke_cmd {
      r#type: RIND_INVOKE_TYPE::ERROR,
      action: null_mut(),
      payload: null_mut(),
    };
  };

  let Ok(msg) = Message::read_signed(&mut stream) else {
    return rind_invoke_cmd {
      r#type: RIND_INVOKE_TYPE::ERROR,
      action: null_mut(),
      payload: null_mut(),
    };
  };

  rind_invoke_cmd {
    action: {
      let str = CString::new(msg.action).unwrap();
      str.into_raw()
    },
    payload: if let Some(p) = msg.payload {
      let str = CString::new(deser_string(p)).unwrap();
      str.into_raw()
    } else {
      null_mut()
    },
    r#type: match msg.r#type {
      rind_ipc::MessageType::Enquire => RIND_INVOKE_TYPE::ENQUIRE,
      rind_ipc::MessageType::Ok => RIND_INVOKE_TYPE::OK,
      rind_ipc::MessageType::Error => RIND_INVOKE_TYPE::ERROR,
      rind_ipc::MessageType::Valid => RIND_INVOKE_TYPE::VALID,
      rind_ipc::MessageType::RequestInput => RIND_INVOKE_TYPE::REQUEST_INPUT,
      rind_ipc::MessageType::Unknown => RIND_INVOKE_TYPE::UNKNOWN,
    },
  }
}
