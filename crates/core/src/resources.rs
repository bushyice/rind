use std::{
  collections::{HashMap, HashSet},
  os::fd::OwnedFd,
};

use nix::sys::timerfd::TimerFd;

use crate::{
  runtime::RuntimePayload,
  types::{ToUstr, Ustr},
};

pub enum FdLoc {
  Owned(OwnedFd),
  Timer(TimerFd),
}

impl From<OwnedFd> for FdLoc {
  fn from(value: OwnedFd) -> Self {
    FdLoc::Owned(value)
  }
}

impl From<TimerFd> for FdLoc {
  fn from(value: TimerFd) -> Self {
    FdLoc::Timer(value)
  }
}

#[derive(Default)]
pub struct Resources {
  actions: HashMap<i32, ResourceAction>,
  fd: HashMap<i32, FdLoc>,
  unwatched_fds: HashSet<i32>,
  watched_fds: HashSet<i32>,
  removed_fds: HashSet<i32>,
  paused_fds: HashSet<i32>,
}

pub struct ResourceAction {
  pub runtime: Ustr,
  pub action: Ustr,
  pub payload: Option<Box<dyn Fn(RuntimePayload) -> RuntimePayload>>,
}

impl ResourceAction {
  pub fn payload(
    mut self,
    payload_constructor: impl Fn(RuntimePayload) -> RuntimePayload + 'static,
  ) -> Self {
    self.payload = Some(Box::new(payload_constructor));
    self
  }
}

impl From<(&str, &str)> for ResourceAction {
  fn from((runtime, action): (&str, &str)) -> Self {
    Self {
      action: action.to_ustr(),
      runtime: runtime.to_ustr(),
      payload: None,
    }
  }
}

impl Resources {
  pub fn register_resource(&mut self, res: i32) {
    if !self.watched_fds.contains(&res) {
      self.unwatched_fds.insert(res);
    }
  }

  pub fn watch(&mut self, res: i32) {
    self.unwatched_fds.remove(&res);
    self.watched_fds.insert(res);
  }

  pub fn unwatched_fds(&self) -> Vec<i32> {
    self.unwatched_fds.iter().copied().collect()
  }

  pub fn removed_fds(&self) -> Vec<i32> {
    self.removed_fds.iter().copied().collect()
  }

  pub fn action(&mut self, res: i32, act: impl Into<ResourceAction>) {
    self.actions.insert(res, act.into());
    self.register_resource(res);
  }

  pub fn get_action(&self, res: i32) -> Option<&ResourceAction> {
    self.actions.get(&res)
  }

  pub fn pause(&mut self, res: i32) {
    if self.watched_fds.remove(&res) {
      self.removed_fds.insert(res);
      self.paused_fds.insert(res);
    }
  }

  pub fn resume(&mut self, res: i32) {
    if self.paused_fds.remove(&res) {
      self.removed_fds.remove(&res);
      self.register_resource(res);
    }
  }

  pub fn is_paused(&self, res: i32) -> bool {
    self.paused_fds.contains(&res)
  }

  pub fn clear_removed(&mut self, res: i32) {
    self.removed_fds.remove(&res);
  }

  pub fn own(&mut self, res: i32, fd: impl Into<FdLoc>) {
    self.fd.insert(res, fd.into());
  }

  pub fn terminate(&mut self, res: i32) {
    self.unwatched_fds.remove(&res);
    if self.watched_fds.remove(&res) {
      self.removed_fds.insert(res);
    };
    self.actions.remove(&res);
  }

  pub fn remove_full(&mut self, res: i32) {
    self.removed_fds.remove(&res);
    self.fd.remove(&res);
  }
}
