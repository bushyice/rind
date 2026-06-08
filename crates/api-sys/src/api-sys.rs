#![allow(non_camel_case_types)]

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use rind_api::{
  InvokeCommand, InvokeType, Message, MessageAction, MessageType, Payload, PayloadType, Transport,
  TransportMethod, invoke as api_invoke, set_sock_path,
};

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RIND_TP_METHOD {
  STDIO = 0,
  UDS = 1,
  SHM = 2,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RIND_MSG_ACTION {
  REMOVE = 0,
  SET = 1,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RIND_MSG_TYPE {
  IMPULSE = 0,
  FACET = 1,
  ENQUIRY = 2,
  RESPONSE = 3,
  UNKNOWN = 4,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RIND_INVOKE_TYPE {
  VALID = 0,
  OK = 1,
  ERROR = 2,
  UNKNOWN = 3,
  REQUEST_INPUT = 4,
  ENQUIRE = 5,
}

#[repr(C)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum RIND_PAYLOAD_TYPE {
  STRING = 0,
  JSON = 1,
}

#[repr(C)]
pub struct rind_msg {
  _unused: [u8; 0],
}
#[repr(C)]
pub struct rind_payload {
  _unused: [u8; 0],
}
#[repr(C)]
pub struct rind_invoke_cmd {
  _unused: [u8; 0],
}
#[repr(C)]
pub struct rind_tp {
  _unused: [u8; 0],
}

fn to_msg_type(t: RIND_MSG_TYPE) -> MessageType {
  match t {
    RIND_MSG_TYPE::IMPULSE => MessageType::Impulse,
    RIND_MSG_TYPE::FACET => MessageType::Facet,
    RIND_MSG_TYPE::ENQUIRY => MessageType::Enquiry,
    RIND_MSG_TYPE::RESPONSE => MessageType::Response,
    RIND_MSG_TYPE::UNKNOWN => MessageType::Unknown,
  }
}

fn to_msg_action(a: RIND_MSG_ACTION) -> MessageAction {
  match a {
    RIND_MSG_ACTION::REMOVE => MessageAction::Remove,
    RIND_MSG_ACTION::SET => MessageAction::Set,
  }
}

fn to_payload_type(t: RIND_PAYLOAD_TYPE) -> PayloadType {
  match t {
    RIND_PAYLOAD_TYPE::STRING => PayloadType::String,
    RIND_PAYLOAD_TYPE::JSON => PayloadType::Json,
  }
}

fn to_invoke_type(t: RIND_INVOKE_TYPE) -> InvokeType {
  match t {
    RIND_INVOKE_TYPE::VALID => InvokeType::Valid,
    RIND_INVOKE_TYPE::OK => InvokeType::Ok,
    RIND_INVOKE_TYPE::ERROR => InvokeType::Error,
    RIND_INVOKE_TYPE::UNKNOWN => InvokeType::Unknown,
    RIND_INVOKE_TYPE::REQUEST_INPUT => InvokeType::RequestInput,
    RIND_INVOKE_TYPE::ENQUIRE => InvokeType::Enquire,
  }
}

fn from_msg_type(t: MessageType) -> RIND_MSG_TYPE {
  match t {
    MessageType::Impulse => RIND_MSG_TYPE::IMPULSE,
    MessageType::Facet => RIND_MSG_TYPE::FACET,
    MessageType::Enquiry => RIND_MSG_TYPE::ENQUIRY,
    MessageType::Response => RIND_MSG_TYPE::RESPONSE,
    MessageType::Unknown => RIND_MSG_TYPE::UNKNOWN,
  }
}

fn from_msg_action(a: MessageAction) -> RIND_MSG_ACTION {
  match a {
    MessageAction::Remove => RIND_MSG_ACTION::REMOVE,
    MessageAction::Set => RIND_MSG_ACTION::SET,
  }
}

fn from_payload_type(t: PayloadType) -> RIND_PAYLOAD_TYPE {
  match t {
    PayloadType::String => RIND_PAYLOAD_TYPE::STRING,
    PayloadType::Json => RIND_PAYLOAD_TYPE::JSON,
  }
}

fn from_invoke_type(t: InvokeType) -> RIND_INVOKE_TYPE {
  match t {
    InvokeType::Valid => RIND_INVOKE_TYPE::VALID,
    InvokeType::Ok => RIND_INVOKE_TYPE::OK,
    InvokeType::Error => RIND_INVOKE_TYPE::ERROR,
    InvokeType::Unknown => RIND_INVOKE_TYPE::UNKNOWN,
    InvokeType::RequestInput => RIND_INVOKE_TYPE::REQUEST_INPUT,
    InvokeType::Enquire => RIND_INVOKE_TYPE::ENQUIRE,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_msg_get_type(msg: *const rind_msg) -> RIND_MSG_TYPE {
  if msg.is_null() {
    return RIND_MSG_TYPE::UNKNOWN;
  }
  let m = unsafe { &*(msg as *const Message) };
  from_msg_type(m.r#type)
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_msg_get_action(msg: *const rind_msg) -> RIND_MSG_ACTION {
  if msg.is_null() {
    return RIND_MSG_ACTION::REMOVE;
  }
  let m = unsafe { &*(msg as *const Message) };
  from_msg_action(m.action)
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_msg_get_name(msg: *const rind_msg) -> *mut c_char {
  if msg.is_null() {
    return ptr::null_mut();
  }
  let m = unsafe { &*(msg as *const Message) };
  match &m.name {
    Some(s) => CString::new(s.as_str()).unwrap().into_raw(),
    None => ptr::null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_msg_get_payload(msg: *const rind_msg) -> *const rind_payload {
  if msg.is_null() {
    return ptr::null();
  }
  let m = unsafe { &*(msg as *const Message) };
  match &m.payload {
    Some(p) => p as *const Payload as *const rind_payload,
    None => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_payload_get_type(payload: *const rind_payload) -> RIND_PAYLOAD_TYPE {
  if payload.is_null() {
    return RIND_PAYLOAD_TYPE::STRING;
  }
  let p = unsafe { &*(payload as *const Payload) };
  from_payload_type(p.r#type)
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_payload_get_content(payload: *const rind_payload) -> *mut c_char {
  if payload.is_null() {
    return ptr::null_mut();
  }
  let p = unsafe { &*(payload as *const Payload) };
  CString::new(p.content.as_str()).unwrap().into_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_invoke_cmd_get_type(cmd: *const rind_invoke_cmd) -> RIND_INVOKE_TYPE {
  if cmd.is_null() {
    return RIND_INVOKE_TYPE::UNKNOWN;
  }
  let c = unsafe { &*(cmd as *const InvokeCommand) };
  from_invoke_type(c.r#type)
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_invoke_cmd_get_action(cmd: *const rind_invoke_cmd) -> *mut c_char {
  if cmd.is_null() {
    return ptr::null_mut();
  }
  let c = unsafe { &*(cmd as *const InvokeCommand) };

  match &c.action {
    Some(s) => CString::new(s.as_str()).unwrap().into_raw(),
    None => ptr::null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_invoke_cmd_get_payload(cmd: *const rind_invoke_cmd) -> *mut c_char {
  if cmd.is_null() {
    return ptr::null_mut();
  }
  let c = unsafe { &*(cmd as *const InvokeCommand) };
  match &c.payload {
    Some(s) => CString::new(s.as_str()).unwrap().into_raw(),
    None => ptr::null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_free_string(ptr: *mut c_char) {
  if !ptr.is_null() {
    unsafe { drop(CString::from_raw(ptr)) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_free_msg(ptr: *mut rind_msg) {
  if !ptr.is_null() {
    unsafe { drop(Box::from_raw(ptr as *mut Message)) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_free_payload(ptr: *mut rind_payload) {
  if !ptr.is_null() {
    unsafe { drop(Box::from_raw(ptr as *mut Payload)) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_free_invoke(ptr: *mut rind_invoke_cmd) {
  if !ptr.is_null() {
    unsafe { drop(Box::from_raw(ptr as *mut InvokeCommand)) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_free_tp(ptr: *mut rind_tp) {
  if !ptr.is_null() {
    unsafe { drop(Box::from_raw(ptr as *mut Transport)) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_init_tp(protocol: RIND_TP_METHOD, options: *const c_char) -> *mut rind_tp {
  let method = match protocol {
    RIND_TP_METHOD::STDIO => TransportMethod::Stdio,
    RIND_TP_METHOD::UDS => TransportMethod::Uds,
    RIND_TP_METHOD::SHM => TransportMethod::Shm,
  };

  let opts: Vec<&str> = if options.is_null() {
    Vec::new()
  } else {
    let input = match unsafe { CStr::from_ptr(options) }.to_str() {
      Ok(s) => s,
      Err(_) => return ptr::null_mut(),
    };
    input.split(' ').collect()
  };

  match Transport::init(method, &opts) {
    Ok(t) => Box::into_raw(Box::new(t)) as *mut rind_tp,
    Err(_) => ptr::null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_listen_tp(tp: *mut rind_tp, func: unsafe extern "C" fn(*mut rind_msg)) {
  if tp.is_null() {
    return;
  }

  let tp = unsafe { &*(tp as *mut Transport) };
  tp.listen(move |msg| {
    let raw = Box::into_raw(Box::new(msg)) as *mut rind_msg;
    unsafe { func(raw) };
  });
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_enquiry_tp(tp: *const rind_tp, message: *const rind_msg) -> *mut rind_msg {
  if tp.is_null() || message.is_null() {
    return ptr::null_mut();
  }

  let tp = unsafe { &*(tp as *const Transport) };
  let msg = unsafe { &*(message as *const Message) };

  match tp.enquiry(msg) {
    Ok(resp) => Box::into_raw(Box::new(resp)) as *mut rind_msg,
    Err(_) => ptr::null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_send_message(tp: *const rind_tp, message: *const rind_msg) -> u8 {
  if tp.is_null() || message.is_null() {
    return 1;
  }

  let tp = unsafe { &*(tp as *const Transport) };
  let msg = unsafe { &*(message as *const Message) };

  match tp.send(msg) {
    Ok(()) => 0,
    Err(_) => 1,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_create_msg(r#type: RIND_MSG_TYPE, action: RIND_MSG_ACTION) -> *mut rind_msg {
  let msg = Message {
    r#type: to_msg_type(r#type),
    action: to_msg_action(action),
    payload: None,
    name: None,
  };
  Box::into_raw(Box::new(msg)) as *mut rind_msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_create_msg_payload(
  r#type: RIND_PAYLOAD_TYPE,
  inner: *const c_char,
) -> *mut rind_payload {
  if inner.is_null() {
    return ptr::null_mut();
  }

  let content = match unsafe { CStr::from_ptr(inner) }.to_str() {
    Ok(s) => s.to_string(),
    Err(_) => return ptr::null_mut(),
  };

  let payload = Payload {
    r#type: to_payload_type(r#type),
    content,
  };

  Box::into_raw(Box::new(payload)) as *mut rind_payload
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_message_payload(message: *mut rind_msg, payload: *mut rind_payload) {
  if message.is_null() || payload.is_null() {
    return;
  }

  let msg = unsafe { &mut *(message as *mut Message) };
  let p = unsafe { *Box::from_raw(payload as *mut Payload) };
  msg.payload = Some(p);
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_message_name(message: *mut rind_msg, name: *const c_char) {
  if message.is_null() {
    return;
  }

  let msg = unsafe { &mut *(message as *mut Message) };
  if name.is_null() {
    msg.name = None;
  } else {
    msg.name = unsafe { CStr::from_ptr(name) }
      .to_str()
      .ok()
      .map(|s| s.to_string());
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_facet(name: *const c_char, payload: *mut rind_payload) -> *mut rind_msg {
  if name.is_null() || payload.is_null() {
    return ptr::null_mut();
  }

  let name_str = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
  let p = unsafe { *Box::from_raw(payload as *mut Payload) };

  Box::into_raw(Box::new(Message::set_facet(name_str, p))) as *mut rind_msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_remove_facet(
  name: *const c_char,
  payload: *mut rind_payload,
) -> *mut rind_msg {
  if name.is_null() {
    return ptr::null_mut();
  }

  let name_str = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
  let p = if payload.is_null() {
    None
  } else {
    Some(unsafe { *Box::from_raw(payload as *mut Payload) })
  };

  Box::into_raw(Box::new(Message::remove_facet(name_str, p))) as *mut rind_msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_impulse(name: *const c_char, payload: *mut rind_payload) -> *mut rind_msg {
  if name.is_null() {
    return ptr::null_mut();
  }

  let name_str = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
  let p = if payload.is_null() {
    None
  } else {
    Some(unsafe { *Box::from_raw(payload as *mut Payload) })
  };

  Box::into_raw(Box::new(Message::impulse(name_str, p))) as *mut rind_msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_log_msg(log: *const c_char) -> *mut rind_msg {
  let s = if log.is_null() {
    ""
  } else {
    unsafe { CStr::from_ptr(log) }.to_str().unwrap_or("")
  };

  Box::into_raw(Box::new(Message::log(s))) as *mut rind_msg
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_create_invoke(
  r#type: RIND_INVOKE_TYPE,
  action: *const c_char,
  payload: *const c_char,
) -> *mut rind_invoke_cmd {
  let action_str = if action.is_null() {
    None
  } else {
    unsafe { CStr::from_ptr(action) }
      .to_str()
      .ok()
      .map(|s| s.to_string())
  };

  let payload_str = if payload.is_null() {
    None
  } else {
    unsafe { CStr::from_ptr(payload) }
      .to_str()
      .ok()
      .map(|s| s.to_string())
  };

  let cmd = InvokeCommand {
    r#type: to_invoke_type(r#type),
    action: action_str,
    payload: payload_str,
  };

  Box::into_raw(Box::new(cmd)) as *mut rind_invoke_cmd
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_set_sock_path(path: *const c_char) {
  if path.is_null() {
    return;
  }
  if let Ok(s) = unsafe { CStr::from_ptr(path) }.to_str() {
    set_sock_path(s);
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn rind_invoke(command: *const rind_invoke_cmd) -> *mut rind_invoke_cmd {
  if command.is_null() {
    return ptr::null_mut();
  }

  let cmd = unsafe { &*(command as *const InvokeCommand) };
  let result = api_invoke(cmd);
  Box::into_raw(Box::new(result)) as *mut rind_invoke_cmd
}
