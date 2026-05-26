use std::{
  io::{Read, Write},
  os::{
    fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    raw::c_void,
    unix::net::UnixStream,
  },
  ptr::NonNull,
  sync::atomic::{AtomicU32, Ordering},
};

use nix::sys::{
  eventfd::EventFd,
  mman::{MapFlags, ProtFlags},
  socket::{ControlMessageOwned, MsgFlags, recvmsg},
};
use rind_core::error::{CoreError, CoreResult};

#[repr(C)]
pub struct ShmHeader {
  pub head: AtomicU32,
  pub tail: AtomicU32,
  pub capacity: u32,
}

pub struct ShmRingBuffer {
  ptr: *mut u8,
}

unsafe impl Send for ShmRingBuffer {}
unsafe impl Sync for ShmRingBuffer {}

impl ShmRingBuffer {
  pub unsafe fn new(ptr: *mut u8) -> Self {
    Self { ptr }
  }

  pub fn ptr(&self) -> *mut u8 {
    self.ptr
  }

  fn header(&self) -> &ShmHeader {
    unsafe { &*(self.ptr as *const ShmHeader) }
  }

  pub fn write(&self, data: &[u8]) -> bool {
    let header = self.header();
    let head = header.head.load(Ordering::Acquire);
    let tail = header.tail.load(Ordering::Acquire);

    let len = data.len() as u32;
    let total_len = len + 4;

    let capacity = header.capacity;
    let data_start = std::mem::size_of::<ShmHeader>() as u32;
    let buffer_size = capacity - data_start;

    let used = if head >= tail {
      head - tail
    } else {
      (u32::MAX - tail) + head + 1
    };

    if used + total_len >= buffer_size {
      return false;
    }

    let mut current_head = head;
    current_head = self.copy_at(current_head, &len.to_ne_bytes(), buffer_size, data_start);
    current_head = self.copy_at(current_head, data, buffer_size, data_start);

    header.head.store(current_head, Ordering::Release);
    true
  }

  pub fn read(&self) -> Option<Vec<u8>> {
    let header = self.header();
    let head = header.head.load(Ordering::Acquire);
    let mut tail = header.tail.load(Ordering::Acquire);

    if tail == head {
      return None;
    }

    let data_start = std::mem::size_of::<ShmHeader>() as u32;
    let capacity = header.capacity;
    let buffer_size = capacity - data_start;

    let mut len_bytes = [0u8; 4];
    tail = self.read_at(tail, &mut len_bytes, buffer_size, data_start);
    let len = u32::from_ne_bytes(len_bytes);

    let mut data = vec![0u8; len as usize];
    tail = self.read_at(tail, &mut data, buffer_size, data_start);

    header.tail.store(tail, Ordering::Release);
    Some(data)
  }

  fn copy_at(&self, offset: u32, src: &[u8], buffer_size: u32, data_start: u32) -> u32 {
    let len = src.len() as u32;
    let pos = offset % buffer_size;
    let space_to_end = buffer_size - pos;

    if len <= space_to_end {
      unsafe {
        std::ptr::copy_nonoverlapping(
          src.as_ptr(),
          self.ptr.add((data_start + pos) as usize),
          len as usize,
        );
      }
    } else {
      unsafe {
        std::ptr::copy_nonoverlapping(
          src.as_ptr(),
          self.ptr.add((data_start + pos) as usize),
          space_to_end as usize,
        );
        std::ptr::copy_nonoverlapping(
          src.as_ptr().add(space_to_end as usize),
          self.ptr.add(data_start as usize),
          (len - space_to_end) as usize,
        );
      }
    }
    offset.wrapping_add(len)
  }

  fn read_at(&self, offset: u32, dest: &mut [u8], buffer_size: u32, data_start: u32) -> u32 {
    let len = dest.len() as u32;
    let pos = offset % buffer_size;
    let space_to_end = buffer_size - pos;

    if len <= space_to_end {
      unsafe {
        std::ptr::copy_nonoverlapping(
          self.ptr.add((data_start + pos) as usize),
          dest.as_mut_ptr(),
          len as usize,
        );
      }
    } else {
      unsafe {
        std::ptr::copy_nonoverlapping(
          self.ptr.add((data_start + pos) as usize),
          dest.as_mut_ptr(),
          space_to_end as usize,
        );
        std::ptr::copy_nonoverlapping(
          self.ptr.add(data_start as usize),
          dest.as_mut_ptr().add(space_to_end as usize),
          (len - space_to_end) as usize,
        );
      }
    }
    offset.wrapping_add(len)
  }
}

pub struct ShmStream {
  pub evt: EventFd,
  pub ring: ShmRingBuffer,
  read_cache: Vec<u8>,
}

impl Default for ShmStream {
  fn default() -> Self {
    unsafe {
      Self {
        evt: EventFd::new().unwrap(),
        ring: ShmRingBuffer::new(std::ptr::null_mut()),
        read_cache: Default::default(),
      }
    }
  }
}

pub struct ShmChannel {
  pub ingress: Option<ShmStream>,
  pub egress: ShmStream,
  _shm_fd: OwnedFd,
  mmap_ptr: *mut u8,
  mmap_size: usize,
}

impl Drop for ShmChannel {
  fn drop(&mut self) {
    unsafe {
      let _ = nix::sys::mman::munmap(
        NonNull::new_unchecked(self.mmap_ptr as *mut c_void),
        self.mmap_size,
      );
    }
  }
}

pub fn shm_client_connect(size: usize, path: &str) -> CoreResult<ShmChannel> {
  let stream = UnixStream::connect(path)?;

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
    if let ControlMessageOwned::ScmRights(f) = cmsg {
      fds.extend(f);
    }
  }

  if fds.len() < 3 {
    return Err(CoreError::InvalidState(format!(
      "didn't receive enough fds (expected 3, got {})",
      fds.len()
    )));
  }

  let shm_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
  let ingress_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
  let egress_fd = unsafe { OwnedFd::from_raw_fd(fds[2]) };

  let total_size = size * 2;
  let ptr = unsafe {
    nix::sys::mman::mmap(
      None,
      std::num::NonZeroUsize::new(total_size).unwrap(),
      ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
      MapFlags::MAP_SHARED,
      &shm_fd,
      0,
    )
  }?;

  let mmap_ptr = ptr.as_ptr() as *mut u8;

  let evt_ingress = unsafe { EventFd::from_owned_fd(ingress_fd) };
  let evt_egress = unsafe { EventFd::from_owned_fd(egress_fd) };

  let ring_ingress = unsafe { ShmRingBuffer::new(mmap_ptr) };
  let ring_egress = unsafe { ShmRingBuffer::new(mmap_ptr.add(size)) };

  Ok(ShmChannel {
    ingress: Some(ShmStream {
      evt: evt_ingress,
      ring: ring_ingress,
      ..Default::default()
    }),
    egress: ShmStream {
      evt: evt_egress,
      ring: ring_egress,
      ..Default::default()
    },
    _shm_fd: shm_fd,
    mmap_ptr,
    mmap_size: total_size,
  })
}

unsafe impl Send for ShmChannel {}
unsafe impl Sync for ShmChannel {}

impl ShmChannel {
  pub fn take_ingress(&mut self) -> Option<ShmStream> {
    self.ingress.take()
  }
}

impl Write for ShmChannel {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    self.egress.write(buf)
  }

  fn flush(&mut self) -> std::io::Result<()> {
    self.egress.flush()
  }
}

impl Read for ShmChannel {
  fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    let Some(ingress) = &mut self.ingress else {
      return Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        "stream ingress not available",
      ));
    };

    ingress.read(buf)
  }
}

impl Read for ShmStream {
  fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
    if self.read_cache.is_empty() {
      if let Err(e) = self.evt.read() {
        return Err(std::io::Error::new(
          std::io::ErrorKind::Other,
          format!("EventFd wake-up read error: {e}"),
        ));
      }
      while let Some(mut data) = self.ring.read() {
        self.read_cache.append(&mut data);
      }

      if self.read_cache.is_empty() {
        return Err(std::io::Error::new(
          std::io::ErrorKind::WouldBlock,
          "Spurious wake-up, no data",
        ));
      }
    }

    let bytes_to_copy = std::cmp::min(buf.len(), self.read_cache.len());
    buf[..bytes_to_copy].copy_from_slice(&self.read_cache[..bytes_to_copy]);

    self.read_cache.drain(..bytes_to_copy);

    Ok(bytes_to_copy)
  }
}

impl Write for ShmStream {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    if buf.is_empty() {
      return Ok(0);
    }

    if self.ring.write(buf) {
      self.evt.write(1)?;
      Ok(buf.len())
    } else {
      Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        "shm ring buffer full",
      ))
    }
  }

  fn flush(&mut self) -> std::io::Result<()> {
    Ok(())
  }
}
