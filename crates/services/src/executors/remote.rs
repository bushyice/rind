use crate::executors::{Executor, ExecutorContext, InstanceHandle};
use nix::sys::signal::Signal;
use rind_core::prelude::*;
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

pub struct RemoteHandle {
  pub tx: Sender<Vec<u8>>,
  pub stdout_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
  pub stderr_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
  pub exit_rx: Arc<Mutex<Receiver<i32>>>,
  pub remote_pid: Option<u32>,
}

impl InstanceHandle for RemoteHandle {
  fn pid(&self) -> Option<u32> {
    self.remote_pid
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

pub struct RemoteExecutor;

impl Executor for RemoteExecutor {
  fn name(&self) -> &'static str {
    "remote"
  }

  fn spawn(&self, ctx: ExecutorContext) -> CoreResult<Box<dyn InstanceHandle>> {
    let addr = ctx.run.exec.as_str();
    let mut _stream = TcpStream::connect(addr)?;
    Err(CoreError::Custom(
      "RemoteExecutor implementation in progress".into(),
    ))
  }
}
