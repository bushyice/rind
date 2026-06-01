use crate::services::*;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rind_core::notifier::Notifier;
use rind_core::prelude::*;
use rind_flow::FacetGraph;
use rind_primitives::mounts::NamespaceMountEntry;
use rind_primitives::prelude::VariableHeap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::process::Child;

pub mod ima;
pub mod native;
pub mod remote;

pub use ima::ImaExecutor;
pub use native::NativeExecutor;
pub use remote::RemoteExecutor;

pub trait InstanceHandle: Send + Sync {
  fn pid(&self) -> Option<u32>;
  fn kill(&mut self, signal: Signal) -> CoreResult<Void>;
  fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>>;
  fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>>;
  fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>>;
}

pub struct ProcessHandle(pub Child);

impl InstanceHandle for ProcessHandle {
  fn pid(&self) -> Option<u32> {
    Some(self.0.id())
  }

  fn kill(&mut self, signal: Signal) -> CoreResult<Void> {
    let pid = Pid::from_raw(-(self.0.id() as i32));
    kill(pid, signal).map_err(|e| CoreError::System(e))
  }

  fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
    self
      .0
      .stdout
      .take()
      .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)
  }

  fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
    self
      .0
      .stderr
      .take()
      .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)
  }

  fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
    self
      .0
      .stdin
      .take()
      .map(|s| Box::new(s) as Box<dyn std::io::Write + Send>)
  }
}

pub struct SupervisorHandle {
  pub pid: u32,
  pub stdout: Option<File>,
  pub stderr: Option<File>,
  pub stdin: Option<File>,
  pub _namespace_fds: Vec<File>,
}

impl InstanceHandle for SupervisorHandle {
  fn pid(&self) -> Option<u32> {
    Some(self.pid)
  }

  fn kill(&mut self, signal: Signal) -> CoreResult<Void> {
    let pid = Pid::from_raw(-(self.pid as i32));
    kill(pid, signal).map_err(CoreError::System)
  }

  fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
    self
      .stdout
      .take()
      .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)
  }

  fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>> {
    self
      .stderr
      .take()
      .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)
  }

  fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
    self
      .stdin
      .take()
      .map(|s| Box::new(s) as Box<dyn std::io::Write + Send>)
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceNetworkConfig {
  pub name: String,
  pub method: String,
  pub address: Option<String>,
  pub gateway: Option<String>,
  pub dns: Option<Vec<String>>,
}

pub struct ExecutorContext<'a> {
  pub service: &'a Service,
  pub run: &'a RunOption,
  pub log: &'a LogHandle,
  pub branch_ctx: Option<&'a ServiceBranchContext>,
  pub sockets_map: &'a HashMap<Ustr, SocketActivation>,
  pub sm: Option<&'a FacetGraph>,
  pub variables: Option<&'a VariableHeap>,
  pub registry_key: Ustr,
  pub notifier: Option<Notifier>,
  pub resources: &'a mut Resources,
  pub resolved_user: Option<Ustr>,
  pub envs: HashMap<Ustr, Ustr>,
  pub args: Vec<Ustr>,
  pub isolation: ServiceIsolation,
  pub cgroup_path: Option<std::path::PathBuf>,
  pub namespace_mounts: Vec<NamespaceMountEntry>,
  pub namespace_networks: Vec<NamespaceNetworkConfig>,
}

pub trait Executor: Send + Sync {
  fn name(&self) -> &'static str;
  fn spawn(&self, ctx: ExecutorContext) -> CoreResult<Box<dyn InstanceHandle>>;
}
