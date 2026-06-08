use nix::sys::epoll::EpollFlags;
use rind_core::resources::{ResourceAction, Resources};

#[test]
fn pause_and_resume_roundtrip() {
  let mut resources = Resources::default();
  resources.action(10, ("runtime", "action"));
  resources.watch(10);

  resources.pause(10);
  assert!(resources.is_paused(10));
  let mut removed = resources.removed_fds();
  removed.sort_unstable();
  assert_eq!(removed, vec![10]);

  resources.resume(10);
  assert!(!resources.is_paused(10));
  assert!(resources.removed_fds().is_empty());
  let mut unwatched = resources.unwatched_fds();
  unwatched.sort_unstable();
  assert_eq!(unwatched, vec![10]);
}

#[test]
fn terminate_cleans_actions_and_marks_removed_when_watched() {
  let mut resources = Resources::default();
  resources.action(42, ("worker", "tick"));
  resources.watch(42);
  resources.flag(42, EpollFlags::EPOLLOUT);
  assert_eq!(resources.flags(42), EpollFlags::EPOLLOUT);

  resources.terminate(42);
  assert!(resources.get_action(42).is_none());
  let mut removed = resources.removed_fds();
  removed.sort_unstable();
  assert_eq!(removed, vec![42]);
  assert_eq!(resources.flags(42), EpollFlags::EPOLLIN);
}

#[test]
fn resource_action_payload_transforms_payload() {
  let action: ResourceAction =
    ResourceAction::from(("runtime", "dispatch")).payload(|p| p.insert("value", 12u32));
  let transform = action.payload.expect("payload transform should exist");
  let mut output = transform(Default::default());
  let value = output.get::<u32>("value").expect("value should be stored");
  assert_eq!(value, 12);
}
