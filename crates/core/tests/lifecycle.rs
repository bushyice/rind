use rind_core::lifecycle::{LifecycleAction, LifecycleQueue};

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
