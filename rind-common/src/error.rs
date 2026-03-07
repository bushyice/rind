use std::fmt::Display;
use std::panic::PanicHookInfo;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::logerr;

fn panic_payload(info: &PanicHookInfo<'_>) -> String {
  if let Some(msg) = info.payload().downcast_ref::<&str>() {
    msg.to_string()
  } else if let Some(msg) = info.payload().downcast_ref::<String>() {
    msg.clone()
  } else {
    "unknown panic payload".to_string()
  }
}

pub fn install_panic_handler(component: &'static str) {
  std::panic::set_hook(Box::new(move |info| {
    let location = info
      .location()
      .map(|l| format!("{}:{}", l.file(), l.line()))
      .unwrap_or_else(|| "unknown location".to_string());
    let message = panic_payload(info);
    let bt = std::backtrace::Backtrace::force_capture();

    logerr!("[rind:{component}] panic at {location}: {message}\nbacktrace:\n{bt}");
  }));
}

pub fn report_error(context: &str, err: impl Display) {
  logerr!("[rind:error] {context}: {err}");
}

pub fn rw_read<'a, T>(lock: &'a RwLock<T>, name: &str) -> RwLockReadGuard<'a, T> {
  match lock.read() {
    Ok(guard) => guard,
    Err(poisoned) => {
      report_error(
        name,
        "rwlock poisoned while acquiring read lock; recovering",
      );
      poisoned.into_inner()
    }
  }
}

pub fn rw_write<'a, T>(lock: &'a RwLock<T>, name: &str) -> RwLockWriteGuard<'a, T> {
  match lock.write() {
    Ok(guard) => guard,
    Err(poisoned) => {
      report_error(
        name,
        "rwlock poisoned while acquiring write lock; recovering",
      );
      poisoned.into_inner()
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{rw_read, rw_write};
  use std::sync::{Arc, RwLock};

  #[test]
  fn rw_helpers_recover_from_poison() {
    let lock = Arc::new(RwLock::new(10usize));
    let for_thread = lock.clone();
    let _ = std::thread::spawn(move || {
      let _guard = for_thread.write().unwrap();
      panic!("intentional poison");
    })
    .join();

    {
      let mut guard = rw_write(&lock, "test write");
      *guard += 1;
    }
    let guard = rw_read(&lock, "test read");
    assert_eq!(*guard, 11);
  }
}
