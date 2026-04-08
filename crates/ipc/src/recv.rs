use rind_core::prelude::{PermissionExpr, PermissionId};

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

#[derive(Default, Clone)]
pub struct IpcSource(pub String, pub PermissionExpr);

impl From<(&str, PermissionId)> for IpcSource {
  fn from(value: (&str, PermissionId)) -> Self {
    Self(value.0.into(), PermissionExpr::Perm(value.1))
  }
}

impl From<(&str, Vec<PermissionId>)> for IpcSource {
  fn from(value: (&str, Vec<PermissionId>)) -> Self {
    Self(
      value.0.into(),
      PermissionExpr::Exact(value.1.iter().map(|x| PermissionExpr::from(*x)).collect()),
    )
  }
}

impl From<String> for IpcSource {
  fn from(value: String) -> Self {
    Self(value, PermissionExpr::All)
  }
}

impl From<&str> for IpcSource {
  fn from(value: &str) -> Self {
    Self(value.into(), PermissionExpr::All)
  }
}

#[derive(Default)]
struct IpcSourcemapInner {
  sources: HashMap<String, IpcSource>,
  // command_builder: Vec<Box<dyn FnMut()>>,
}

#[derive(Default, Clone)]
pub struct IpcSourcemap {
  inner: Arc<RwLock<IpcSourcemapInner>>,
}

pub struct IpcSourceBuilder2 {
  builder: IpcSourceBuilder,
  name: String,
  perms: PermissionExpr,
}

impl IpcSourceBuilder2 {
  fn new(builder: IpcSourceBuilder, name: String) -> Self {
    Self {
      name,
      perms: PermissionExpr::All,
      builder,
    }
  }

  pub fn allow(mut self, perm: impl Into<PermissionExpr>) -> IpcSourceBuilder {
    self.perms = perm.into();
    self.allow_all()
  }

  pub fn allow_all(mut self) -> IpcSourceBuilder {
    self.builder.actions.insert(self.name, self.perms);
    self.builder
  }
}

pub struct IpcSourceBuilder {
  srcmap: IpcSourcemap,
  runtime: String,
  actions: HashMap<String, PermissionExpr>,
}

impl IpcSourceBuilder {
  fn new(srcmap: IpcSourcemap, runtime: String) -> Self {
    Self {
      runtime,
      actions: Default::default(),
      srcmap,
    }
  }

  pub fn insert(self, name: impl Into<String>) -> IpcSourceBuilder2 {
    IpcSourceBuilder2::new(self, name.into())
  }

  pub fn build(self) -> IpcSourcemap {
    for (action, perm) in self.actions {
      self
        .srcmap
        .entry(&action, IpcSource(self.runtime.clone(), perm));
    }
    self.srcmap
  }
}

impl IpcSourcemap {
  pub fn entry(&self, action: &str, source: impl Into<IpcSource>) {
    let mut map = self.inner.write().unwrap();
    map.sources.insert(action.into(), source.into());
    drop(map);
  }

  pub fn message(&self, action: &str) -> Option<IpcSource> {
    let map = self.inner.read().unwrap();
    let result = map.sources.get(action).cloned();
    drop(map);
    result
  }

  pub fn build(self, name: impl Into<String>) -> IpcSourceBuilder {
    IpcSourceBuilder::new(self, name.into())
  }
}
