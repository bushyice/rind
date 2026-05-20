use std::error::Error;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::exit;
use std::thread;

use nix::sys::eventfd::EventFd;
use nix::sys::mman::{MapFlags, ProtFlags, mmap};
use nix::sys::socket::{ControlMessageOwned, MsgFlags, recvmsg};

use rind_ipc::TransportMessage;
use rind_ipc::shm::ShmRingBuffer;

const SHM_SIZE: usize = 1024 * 1024;

#[derive(Debug)]
struct ConError(String);

impl std::fmt::Display for ConError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "Error: {}", self.0)
  }
}

impl Error for ConError {}

fn main() -> Result<(), Box<dyn Error>> {
  println!("connesting to shm tp...");
  let mut stream = None;
  for _ in 0..20 {
    match UnixStream::connect("/run/rind-tp/shm.sock") {
      Ok(s) => {
        stream = Some(s);
        break;
      }
      Err(_) => {
        thread::sleep(std::time::Duration::from_millis(100));
      }
    }
  }

  let stream = stream.ok_or_else(|| ConError("failed to connect to SHM transport".into()))?;

  let mut buf = [0u8; 1];
  let mut iov = [std::io::IoSliceMut::new(&mut buf)];
  let mut cmsg_buf = nix::cmsg_space!([RawFd; 3]);
  let msg = recvmsg::<()>(
    stream.as_raw_fd(),
    &mut iov,
    Some(&mut cmsg_buf),
    MsgFlags::empty(),
  )?;

  let mut fds = Vec::new();
  for cmsg in msg.cmsgs()? {
    match cmsg {
      ControlMessageOwned::ScmRights(f) => fds.extend(f),
      _ => {}
    }
  }

  if fds.len() < 3 {
    eprintln!("didn't receive enough fds (expected 3, got {})", fds.len());
    exit(1);
  }

  let shm_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
  let evt_to_client_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
  let evt_to_rind_fd = unsafe { OwnedFd::from_raw_fd(fds[2]) };

  let evt_to_client = unsafe { EventFd::from_owned_fd(evt_to_client_fd) };
  let evt_to_rind = unsafe { EventFd::from_owned_fd(evt_to_rind_fd) };

  println!("mapping shared memory...");
  let ptr = unsafe {
    mmap(
      None,
      std::num::NonZeroUsize::new(SHM_SIZE * 2).unwrap(),
      ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
      MapFlags::MAP_SHARED,
      &shm_fd,
      0,
    )
  }?;

  let ptr = ptr.as_ptr() as *mut u8;

  let ring_to_client = unsafe { ShmRingBuffer::new(ptr) };
  let ring_to_rind = unsafe { ShmRingBuffer::new(ptr.add(SHM_SIZE)) };

  println!("shm tp connected and mapped");

  thread::spawn(move || {
    loop {
      match evt_to_client.read() {
        Ok(_) => {
          while let Some(data) = ring_to_client.read() {
            if let Ok(msg) = flexbuffers::from_slice::<TransportMessage>(&data) {
              println!("Received message: {:?}", msg.name);
            }
          }
        }
        Err(e) => {
          eprintln!("EventFd read error: {e}");
          break;
        }
      }
    }
  });

  println!("sending message to rind...");
  let msg = TransportMessage::log("hello from shm client!");
  let data = msg.as_bytes();
  if ring_to_rind.write(&data) {
    evt_to_rind.write(1)?;
    println!("sent");
  } else {
    println!("failed to write to ring buffer... full?");
  }

  loop {
    thread::sleep(std::time::Duration::from_secs(1));
  }
}

type RawFd = std::os::unix::io::RawFd;
