use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
  ReloadUnits,
  Reboot,
  Shutdown,
}

#[derive(Clone, Default)]
pub struct LifecycleQueue {
  inner: Arc<Mutex<VecDeque<LifecycleAction>>>,
}

impl LifecycleQueue {
  pub fn request(&self, action: LifecycleAction) {
    if let Ok(mut queue) = self.inner.lock() {
      queue.push_back(action);
    }
  }

  pub fn next(&self) -> Option<LifecycleAction> {
    self.inner.lock().ok()?.pop_front()
  }
}
