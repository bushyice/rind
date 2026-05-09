use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
  ReloadUnits,
  SoftReboot,
  Reboot,
  Shutdown,
}

#[derive(Clone, Default)]
pub struct LifecycleQueue {
  inner: Rc<RefCell<VecDeque<LifecycleAction>>>,
}

impl LifecycleQueue {
  pub fn request(&self, action: LifecycleAction) {
    self.inner.borrow_mut().push_back(action);
  }

  pub fn next(&self) -> Option<LifecycleAction> {
    self.inner.borrow_mut().pop_front()
  }
}

#[cfg(test)]
mod tests {
  use super::{LifecycleAction, LifecycleQueue};

  #[test]
  fn lifecycle_queue_is_fifo() {
    let queue = LifecycleQueue::default();
    queue.request(LifecycleAction::ReloadUnits);
    queue.request(LifecycleAction::SoftReboot);
    queue.request(LifecycleAction::Shutdown);

    assert_eq!(queue.next(), Some(LifecycleAction::ReloadUnits));
    assert_eq!(queue.next(), Some(LifecycleAction::SoftReboot));
    assert_eq!(queue.next(), Some(LifecycleAction::Shutdown));
    assert_eq!(queue.next(), None);
  }

  #[test]
  fn lifecycle_queue_is_shared_across_clones() {
    let queue = LifecycleQueue::default();
    let clone = queue.clone();

    queue.request(LifecycleAction::Reboot);
    assert_eq!(clone.next(), Some(LifecycleAction::Reboot));
  }
}
