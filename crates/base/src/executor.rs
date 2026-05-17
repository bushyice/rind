use crate::flow::StateMachine;
use crate::services::{
  RunOption, Service, ServiceBranchContext, ServiceNamespaces, SocketActivation,
};
use crate::variables::VariableHeap;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rind_core::notifier::Notifier;
use rind_core::prelude::*;
use rind_core::utils::read_env_file;
use std::collections::HashMap;
use std::net::TcpStream;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub trait InstanceHandle: Send + Sync {
  fn pid(&self) -> Option<u32>;
  fn kill(&mut self, signal: Signal) -> CoreResult<()>;
  fn take_stdout(&mut self) -> Option<Box<dyn std::io::Read + Send>>;
  fn take_stderr(&mut self) -> Option<Box<dyn std::io::Read + Send>>;
  fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>>;
}

pub struct ProcessHandle(pub Child);

impl InstanceHandle for ProcessHandle {
  fn pid(&self) -> Option<u32> {
    Some(self.0.id())
  }

  fn kill(&mut self, signal: Signal) -> CoreResult<()> {
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

pub struct ExecutorContext<'a> {
  pub service: &'a Service,
  pub run: &'a RunOption,
  pub log: &'a LogHandle,
  pub branch_ctx: Option<&'a ServiceBranchContext>,
  pub sockets_map: &'a HashMap<Ustr, SocketActivation>,
  pub sm: Option<&'a StateMachine>,
  pub variables: Option<&'a VariableHeap>,
  pub registry_key: Ustr,
  pub notifier: Option<Notifier>,
  pub resources: &'a mut Resources,
  pub resolved_user: Option<Ustr>,
  pub envs: HashMap<Ustr, Ustr>,
  pub args: Vec<Ustr>,
}

pub trait Executor: Send + Sync {
  fn name(&self) -> &'static str;
  fn spawn(&self, ctx: ExecutorContext) -> CoreResult<Box<dyn InstanceHandle>>;
}

pub struct NaturalExecutor;

impl NaturalExecutor {
  fn namespace_unshare_flags(ns: &ServiceNamespaces) -> libc::c_int {
    let mut flags = 0;
    if ns.mount {
      flags |= libc::CLONE_NEWNS;
    }
    if ns.uts {
      flags |= libc::CLONE_NEWUTS;
    }
    if ns.ipc {
      flags |= libc::CLONE_NEWIPC;
    }
    if ns.net {
      flags |= libc::CLONE_NEWNET;
    }
    if ns.cgroup {
      flags |= libc::CLONE_NEWCGROUP;
    }
    flags
  }
}

impl Executor for NaturalExecutor {
  fn name(&self) -> &'static str {
    "natural"
  }

  fn spawn(&self, ctx: ExecutorContext) -> CoreResult<Box<dyn InstanceHandle>> {
    let args = ctx.args.clone();
    let mut envs = ctx.envs.clone();
    let branch_key = ctx.branch_ctx.and_then(|c| c.key.as_ref());

    let mut cmd = Command::new(ctx.run.exec.as_str());
    let pre_exec_fds = ctx
      .sockets_map
      .get(&rslvns!(snorm ctx.registry_key).to_ustr())
      .map(|s| s.fds.clone())
      .unwrap_or_default();

    println!("{pre_exec_fds:?} {}", ctx.registry_key);

    let ns_flags = ctx
      .service
      .metadata
      .namespaces
      .as_ref()
      .map(Self::namespace_unshare_flags)
      .unwrap_or(0);

    cmd
      .args(args.iter().map(|a| a.as_str()))
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped());

    unsafe {
      cmd.pre_exec(move || {
        libc::setsid();
        if ns_flags != 0 && libc::unshare(ns_flags) < 0 {
          return Err(std::io::Error::last_os_error());
        }
        for (idx, fd) in pre_exec_fds.iter().enumerate() {
          let target_fd = (3 + idx) as RawFd;
          if *fd != target_fd && libc::dup2(*fd, target_fd) < 0 {
            return Err(std::io::Error::last_os_error());
          }
          let flags = libc::fcntl(target_fd, libc::F_GETFD);
          if flags >= 0 {
            let _ = libc::fcntl(target_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
          }
        }
        Ok(())
      });
    }

    let user_info = if let Some(username) = ctx.resolved_user.as_ref() {
      let store = rind_core::user::UserStore::load_system()?;
      let Some(user) = store.lookup_by_name(username.as_str()) else {
        return Err(CoreError::InvalidState(format!(
          "user '{username}' not found"
        )));
      };
      Some((user.uid, user.gid, user.home.clone(), username.clone()))
    } else {
      None
    };

    if let Some(dir) = &ctx.service.metadata.working_dir {
      cmd.current_dir(dir.as_str());
    }

    if let Some((uid, gid, home, username)) = user_info {
      cmd.uid(uid);
      cmd.gid(gid);

      if let Some(dir) = &ctx.service.metadata.working_dir
        && dir.as_str().starts_with("~")
      {
        cmd.current_dir(format!("{}{}", home, &dir.as_str()[1..]));
      }

      envs.extend(
        read_env_file(&format!("{home}/.env"))
          .into_iter()
          .map(|(k, v)| (Ustr::from(k), Ustr::from(v))),
      );

      envs.insert(Ustr::from("HOME"), Ustr::from(home));
      envs.insert(Ustr::from("USER"), username);
    }

    if let Some(key) = branch_key {
      cmd.env("RIND_BRANCH_KEY", key.as_str());
    }

    if !envs.is_empty() {
      cmd.envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let child = cmd.spawn()?;
    Ok(Box::new(ProcessHandle(child)))
  }
}

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

  fn kill(&mut self, _signal: Signal) -> CoreResult<()> {
    Ok(())
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

pub struct ImaHandle {
  pub join_handle: Option<thread::JoinHandle<CoreResult<()>>>,
  pub stdout_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
  pub stderr_rx: Arc<Mutex<Receiver<Vec<u8>>>>,
  pub stdin_tx: Sender<Vec<u8>>,
}

impl InstanceHandle for ImaHandle {
  fn pid(&self) -> Option<u32> {
    None
  }

  fn kill(&mut self, _signal: Signal) -> CoreResult<()> {
    Ok(())
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
