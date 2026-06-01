use crate::executors::{Executor, ExecutorContext, InstanceHandle};
use nix::sys::signal::Signal;
use rind_core::prelude::*;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct ImaHandle {
  pub join_handle: Option<thread::JoinHandle<CoreResult<Void>>>,
  pub stdout_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
  pub stderr_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
  pub stdin_tx: Sender<Vec<u8>>,
}

impl InstanceHandle for ImaHandle {
  fn pid(&self) -> Option<u32> {
    None
  }

  fn kill(&mut self, _signal: Signal) -> CoreResult<Void> {
    Ok(Void)
  }

  fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
    None
  }

  fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
    None
  }

  fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
    None
  }
}

pub struct ImaExecutor;

impl Executor for ImaExecutor {
  fn name(&self) -> &'static str {
    "ima"
  }

  fn spawn(&self, _ctx: ExecutorContext) -> CoreResult<Box<dyn InstanceHandle>> {
    Err(CoreError::Custom(
      "ImaExecutor implementation in progress".into(),
    ))
  }
}
