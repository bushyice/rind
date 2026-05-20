use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;

use nix::sys::eventfd::{EfdFlags, EventFd};
use nix::sys::memfd::{MFdFlags, memfd_create};
use nix::sys::mman::{MapFlags, ProtFlags, mmap};
use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};
use nix::unistd::ftruncate;
use rind_core::reexports::*;
use std::os::fd::AsRawFd;

use crate::prelude::PermissionStore;
use crate::transport::{TransportProtocol, TransportResponder, socket_path};
use rind_core::notifier::Notifier;
use rind_core::prelude::*;
use rind_ipc::TransportMessage;
use rind_ipc::shm::{ShmHeader, ShmRingBuffer};

const SHM_SIZE: usize = 1024 * 1024;

pub struct ShmClient {
  pub ring_to_client: ShmRingBuffer,
  pub ring_to_rind: ShmRingBuffer,
  pub evt_to_client: EventFd,
  pub evt_to_rind: EventFd,
  #[allow(dead_code)]
  pub uid: u32,
}

pub struct ShmTransport {
  pub clients: Arc<Mutex<HashMap<Ustr, Vec<Arc<ShmClient>>>>>,
  pub started: std::collections::HashSet<Ustr>,
  pub incoming_tx:
    std::sync::mpsc::Sender<(Ustr, TransportMessage, u32, Option<TransportResponder>)>,
  pub incoming_rx: Arc<
    Mutex<std::sync::mpsc::Receiver<(Ustr, TransportMessage, u32, Option<TransportResponder>)>>,
  >,
}

impl Default for ShmTransport {
  fn default() -> Self {
    let (tx, rx) = std::sync::mpsc::channel();
    Self {
      clients: Arc::new(Mutex::new(HashMap::new())),
      started: std::collections::HashSet::new(),
      incoming_tx: tx,
      incoming_rx: Arc::new(Mutex::new(rx)),
    }
  }
}

impl ShmTransport {
  fn start_listener(
    &self,
    endpoint: Ustr,
    permissions: Option<Vec<Ustr>>,
    pm: Option<PermissionStore>,
    notifier: Option<Notifier>,
  ) {
    let path = socket_path(&endpoint);
    if let Some(parent) = path.parent() {
      let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&path);

    let listener = match std::os::unix::net::UnixListener::bind(&path) {
      Ok(l) => l,
      Err(e) => {
        eprintln!(
          "[transport] shm handshake bind failed {}: {e}",
          path.display()
        );
        return;
      }
    };

    let clients = self.clients.clone();
    let tx = self.incoming_tx.clone();
    let ep = endpoint.clone();

    thread::spawn(move || {
      for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let mut uid = 0;

        if let Some(ref permissions) = permissions
          && let Some(ref pm) = pm
        {
          let Ok(cred) = crate::prelude::get_peer_cred_stream(&stream) else {
            continue;
          };
          uid = cred.uid;

          if !permissions
            .iter()
            .any(|x| pm.from_name(x).map_or(false, |x| pm.user_has(cred.uid, x)))
          {
            continue;
          }
        }

        let shm_fd = match memfd_create("rind-shm", MFdFlags::empty()) {
          Ok(fd) => fd,
          Err(e) => {
            eprintln!("[shm] failed to create memfd: {e}");
            continue;
          }
        };
        let _ = ftruncate(&shm_fd, (SHM_SIZE * 2) as i64);

        let evt_to_client = match EventFd::from_value_and_flags(0, EfdFlags::empty()) {
          Ok(ef) => ef,
          Err(_) => continue,
        };
        let evt_to_rind = match EventFd::from_value_and_flags(0, EfdFlags::empty()) {
          Ok(ef) => ef,
          Err(_) => continue,
        };

        let ptr = unsafe {
          mmap(
            None,
            std::num::NonZeroUsize::new(SHM_SIZE * 2).unwrap(),
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            &shm_fd,
            0,
          )
        };

        let Ok(ptr) = ptr else { continue };
        let ptr = ptr.as_ptr() as *mut u8;

        unsafe {
          let h1 = &mut *(ptr as *mut ShmHeader);
          h1.head.store(0, Ordering::Release);
          h1.tail.store(0, Ordering::Release);
          h1.capacity = SHM_SIZE as u32;

          let h2 = &mut *(ptr.add(SHM_SIZE) as *mut ShmHeader);
          h2.head.store(0, Ordering::Release);
          h2.tail.store(0, Ordering::Release);
          h2.capacity = SHM_SIZE as u32;
        }

        let client = Arc::new(ShmClient {
          ring_to_client: unsafe { ShmRingBuffer::new(ptr) },
          ring_to_rind: unsafe { ShmRingBuffer::new(ptr.add(SHM_SIZE)) },
          evt_to_client,
          evt_to_rind,
          uid,
        });

        let fds = [
          shm_fd.as_raw_fd(),
          client.evt_to_client.as_raw_fd(),
          client.evt_to_rind.as_raw_fd(),
        ];
        let cmsg = [ControlMessage::ScmRights(&fds)];
        let iov = [std::io::IoSlice::new(&[0u8])];
        match sendmsg::<()>(stream.as_raw_fd(), &iov, &cmsg, MsgFlags::empty(), None) {
          Err(e) => eprint!("[shm] failed to send msg: {e}"),
          _ => {}
        }

        if let Ok(mut locked) = clients.lock() {
          locked.entry(ep.clone()).or_default().push(client.clone());
        }

        let tx = tx.clone();
        let ep_for_msg = ep.clone();
        let notifier = notifier.clone();
        let client_rx = client.clone();

        thread::spawn(move || {
          loop {
            match client_rx.evt_to_rind.read() {
              Ok(_) => {
                while let Some(data) = client_rx.ring_to_rind.read() {
                  if let Ok(msg) = flexbuffers::from_slice::<TransportMessage>(&data) {
                    let _ = tx.send((ep_for_msg.clone(), msg, uid, None));
                    if let Some(n) = &notifier {
                      let _ = n.notify();
                    }
                  }
                }
              }
              Err(_) => break,
            }
          }
        });
      }
    });
  }
}

impl TransportProtocol for ShmTransport {
  fn setup(
    &mut self,
    endpoint: &str,
    permissions: Option<Vec<Ustr>>,
    pm: Option<PermissionStore>,
    notifier: Option<Notifier>,
  ) {
    let endpoint = Ustr::from(endpoint);
    if self.started.contains(&endpoint) {
      return;
    }
    self.start_listener(endpoint.clone(), permissions, pm, notifier);
    self.started.insert(endpoint);
  }

  fn send_message(&self, endpoint: &str, msg: &TransportMessage) {
    if let Ok(mut locked) = self.clients.lock() {
      if let Some(clients) = locked.get_mut(endpoint) {
        let data = msg.as_bytes();
        clients.retain(|client| {
          if client.ring_to_client.write(&data) {
            let _ = client.evt_to_client.write(1);
            true
          } else {
            // TODO: maybe add a timeout or health check
            true
          }
        });
      }
    }
  }
}
