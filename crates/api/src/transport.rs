use std::collections::HashMap;
use std::io::{Write, stdin, stdout};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};
use std::thread;

use rind_ipc::ser::deser_from_vec;
use rind_ipc::shm::{ShmChannel, shm_client_connect};
use rind_ipc::{TransportMessage, TransportStream};

use crate::msg::{InvokeCommand, InvokeType, Message};
use rind_ipc::Message as IpcMessage;

const SHM_SIZE: usize = 1024 * 1024;

static UDS_CONNECTIONS: LazyLock<RwLock<HashMap<u64, UnixStream>>> =
  LazyLock::new(|| RwLock::new(HashMap::new()));
static TP_STREAMS: LazyLock<RwLock<HashMap<u64, TransportStream>>> =
  LazyLock::new(|| RwLock::new(HashMap::new()));
static SHM_CONNECTIONS: LazyLock<RwLock<HashMap<u64, ShmChannel>>> =
  LazyLock::new(|| RwLock::new(HashMap::new()));
static TRANSPORT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMethod {
  Stdio,
  Uds,
  Shm,
}

pub struct Transport {
  pub(crate) id: u64,
  pub(crate) method: TransportMethod,
}

impl Transport {
  pub fn init(method: TransportMethod, options: &[&str]) -> Result<Self, String> {
    let id = TRANSPORT_ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    if !options.is_empty() {
      let mut retries = 0;
      let mut last = Ok(());

      while retries < 5 {
        match method {
          TransportMethod::Uds => match UnixStream::connect(options[0]) {
            Ok(stream) => {
              UDS_CONNECTIONS.write().unwrap().insert(id, stream);
              last = Ok(());
              break;
            }
            Err(e) => {
              last = Err(format!("Failed to create connection: {e}"));
              if !e.to_string().contains("no such file") {
                break;
              }
            }
          },
          TransportMethod::Shm => match shm_client_connect(SHM_SIZE, options[0]) {
            Ok(conn) => {
              SHM_CONNECTIONS.write().unwrap().insert(id, conn);
              last = Ok(());
              break;
            }
            Err(e) => {
              last = Err(format!("Failed to create connection: {e}"));
              if !e.to_string().contains("no such file") {
                break;
              }
            }
          },
          TransportMethod::Stdio => {
            last = Ok(());
            break;
          }
        }
        retries += 1;
        std::thread::sleep(std::time::Duration::from_millis(100));
      }

      last?;
    }

    Ok(Transport { id, method })
  }

  pub fn listen<F>(&self, mut callback: F)
  where
    F: FnMut(Message) + Send + 'static,
  {
    let id = self.id;
    let method = self.method;

    thread::spawn(move || match method {
      TransportMethod::Stdio => {
        while let Ok(m) = TransportMessage::read_signed(&stdin()) {
          callback(Message::from_transport(m));
        }
      }
      TransportMethod::Uds | TransportMethod::Shm => {
        let mut stream: TransportStream = if method == TransportMethod::Shm {
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

        if method == TransportMethod::Uds {
          while let Ok(m) = TransportMessage::read_signed(&mut stream) {
            callback(Message::from_transport(m));
          }
        } else {
          let stream = stream.as_shm().unwrap();
          loop {
            match stream.evt.read() {
              Ok(_) => {
                while let Some(data) = stream.ring.read() {
                  if let Ok(msg) = deser_from_vec::<TransportMessage>(&data, true) {
                    callback(Message::from_transport(msg));
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

  pub fn send(&self, message: &Message) -> Result<(), String> {
    let msg = message.to_transport();

    match self.method {
      TransportMethod::Stdio => msg.write_signed(stdout()).map_err(|e| e.to_string()),
      TransportMethod::Shm => {
        let mut conns = SHM_CONNECTIONS.write().unwrap();
        let conn = conns.get_mut(&self.id).ok_or("connection not found")?;
        let data = msg.as_bytes();
        conn.write(&data).map(|_| ()).map_err(|e| e.to_string())
      }
      TransportMethod::Uds => {
        let stream = {
          let conns = UDS_CONNECTIONS.read().unwrap();
          let conn = conns.get(&self.id).ok_or("connection not found")?;
          conn.try_clone().map_err(|e| e.to_string())?
        };
        msg.write_signed(stream).map_err(|e| e.to_string())
      }
    }
  }

  pub fn enquiry(&self, message: &Message) -> Result<Message, String> {
    if self.method == TransportMethod::Stdio {
      return Err("STDIO does not support enquiry".into());
    }

    let mut stream: TransportStream = match self.method {
      TransportMethod::Shm => TP_STREAMS
        .write()
        .unwrap()
        .remove(&self.id)
        .map(Ok)
        .unwrap_or(Err("no cached stream".to_string()))?,
      TransportMethod::Uds => TP_STREAMS
        .write()
        .unwrap()
        .remove(&self.id)
        .map(Ok)
        .unwrap_or(Err("no cached stream".to_string()))?,
      TransportMethod::Stdio => unreachable!(),
    };

    let msg = message.to_transport();
    msg.write_signed(&mut stream).map_err(|e| e.to_string())?;

    let result = if self.method == TransportMethod::Shm {
      let shm_stream = stream.as_shm_chan_ref().unwrap();
      let ingress = shm_stream.ingress.as_ref().unwrap();
      match ingress.evt.read() {
        Ok(_) => {
          let mut data = Vec::new();
          while let Some(d) = ingress.ring.read() {
            data = d;
          }
          deser_from_vec::<TransportMessage>(&data, true)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))
        }
        Err(e) => Err(std::io::Error::new(
          std::io::ErrorKind::BrokenPipe,
          format!("{e}"),
        )),
      }
    } else {
      TransportMessage::read_signed(&mut stream)
    };

    match result {
      Ok(m) => {
        TP_STREAMS.write().unwrap().insert(self.id, stream);
        Ok(Message::from_transport(m))
      }
      Err(e) => Err(e.to_string()),
    }
  }
}

static RIND_SOCK_PATH: LazyLock<RwLock<String>> =
  LazyLock::new(|| RwLock::new("/tmp/rind.sock".into()));

pub fn set_sock_path(path: &str) {
  *RIND_SOCK_PATH.write().unwrap() = path.to_string();
}

pub fn invoke(command: &InvokeCommand) -> InvokeCommand {
  let Ok(mut stream) = UnixStream::connect(RIND_SOCK_PATH.read().unwrap().clone()) else {
    return InvokeCommand {
      action: None,
      payload: None,
      r#type: InvokeType::Error,
    };
  };

  let msg = command.to_message();

  let Ok(_) = msg.write_signed(&mut stream) else {
    return InvokeCommand {
      r#type: InvokeType::Error,
      action: None,
      payload: None,
    };
  };

  let Ok(msg) = IpcMessage::read_signed(&mut stream) else {
    return InvokeCommand {
      r#type: InvokeType::Error,
      action: None,
      payload: None,
    };
  };

  InvokeCommand::from_message(msg)
}
