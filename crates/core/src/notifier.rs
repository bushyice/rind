use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::sync::Arc;

#[allow(deprecated)]
use nix::sys::eventfd::{EfdFlags, eventfd};
use nix::unistd::{read, write};

use crate::error::CoreError;

#[derive(Clone)]
pub struct Notifier {
  fd: Arc<OwnedFd>,
}

impl Notifier {
  pub fn new() -> Result<Self, CoreError> {
    #[allow(deprecated)]
    let owned = eventfd(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
      .map_err(|e| CoreError::Custom(format!("failed to create eventfd: {e}")))?;

    Ok(Self {
      fd: Arc::new(owned),
    })
  }

  pub fn as_raw_fd(&self) -> RawFd {
    self.fd.as_raw_fd()
  }

  pub fn notify(&self) -> Result<(), CoreError> {
    let val: u64 = 1;
    let bytes = val.to_ne_bytes();
    write(&*self.fd, &bytes)
      .map_err(|e| CoreError::Custom(format!("failed to write to eventfd: {e}")))?;
    Ok(())
  }

  pub fn reset(&self) -> Result<(), CoreError> {
    let mut buf = [0u8; 8];
    // reset
    match read(&*self.fd, &mut buf) {
      Ok(_) => Ok(()),
      Err(nix::errno::Errno::EAGAIN) => Ok(()),
      Err(e) => Err(CoreError::Custom(format!(
        "failed to read from eventfd: {e}"
      ))),
    }
  }
}

impl AsFd for Notifier {
  fn as_fd(&self) -> BorrowedFd<'_> {
    self.fd.as_fd()
  }
}
