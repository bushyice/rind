use rind_core::prelude::{
  CoreError, LogHandle, PermissionExpr, RuntimeContext, RuntimeDispatcher,
};

use super::Message;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, RwLock};
use std::thread;

type ClientHandler = fn(Message) -> Result<Option<Message>, anyhow::Error>;

pub fn recv_message(mut stream: UnixStream, handle_client: ClientHandler) {
  println!("client connected");

  loop {
    let mut len_buf = [0u8; 4];
    if let Err(e) = stream.read_exact(&mut len_buf) {
      eprintln!("client disconnected / len read error: {e}");
      break;
    }

    let len = u32::from_be_bytes(len_buf) as usize;

    let mut buf = vec![0u8; len];
    if let Err(e) = stream.read_exact(&mut buf) {
      eprintln!("payload read error: {e}");
      break;
    }

    let raw = match String::from_utf8(buf) {
      Ok(s) => s,
      Err(e) => {
        eprintln!("utf8 error: {e}");
        continue;
      }
    };

    let msg: Message = match serde_json::from_str(&raw) {
      Ok(m) => m,
      Err(e) => {
        eprintln!("json parse error: {e}");
        continue;
      }
    };

    let response = match handle_client(msg) {
      Ok(Some(response)) => response,
      Ok(None) => Message::err("no response from handler"),
      Err(err) => Message::err(format!("handler error: {err}")),
    };

    let resp = response.as_string().into_bytes();
    let len = (resp.len() as u32).to_be_bytes();

    if let Err(e) = stream.write_all(&len) {
      eprintln!("write len error: {e}");
      break;
    }

    if let Err(e) = stream.write_all(&resp) {
      eprintln!("write payload error: {e}");
      break;
    }
  }
}

pub fn start_ipc_server(handle_client: ClientHandler) -> std::io::Result<()> {
  let socket_path = "/tmp/rind.sock";
  let _ = std::fs::remove_file(socket_path);
  let listener = UnixListener::bind(socket_path)?;

  println!("Daemon IPC listening on {}", socket_path);

  for stream in listener.incoming() {
    match stream {
      Ok(stream) => {
        thread::spawn(move || recv_message(stream, handle_client));
      }
      Err(e) => eprintln!("IPC connection failed: {}", e),
    }
  }

  Ok(())
}

pub type IpcHandler =
  fn(Message, &mut RuntimeContext<'_>, &RuntimeDispatcher, &LogHandle) -> Result<Message, CoreError>;

#[derive(Clone)]
pub struct IpcSource {
  pub handler: IpcHandler,
  pub perms: PermissionExpr,
}

#[derive(Default)]
struct IpcSourcemapInner {
  sources: HashMap<String, IpcSource>,
}

#[derive(Default, Clone)]
pub struct IpcSourcemap {
  inner: Arc<RwLock<IpcSourcemapInner>>,
}

impl IpcSourcemap {
  pub fn register(
    &self,
    action: impl Into<String>,
    handler: IpcHandler,
    perms: impl Into<PermissionExpr>,
  ) {
    let mut map = self.inner.write().unwrap();
    map.sources.insert(
      action.into(),
      IpcSource {
        handler,
        perms: perms.into(),
      },
    );
    drop(map);
  }

  pub fn message(&self, action: &str) -> Option<IpcSource> {
    let map = self.inner.read().unwrap();
    let result = map.sources.get(action).cloned();
    drop(map);
    result
  }
}
