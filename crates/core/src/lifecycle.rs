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
