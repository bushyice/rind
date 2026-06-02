use crate::executors::{Executor, ExecutorContext, InstanceHandle, ProcessHandle};
use crate::namespaces;
use rind_core::prelude::*;
use rind_core::utils::read_env_file;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::process::Stdio;

pub struct NativeExecutor;

impl Executor for NativeExecutor {
  fn name(&self) -> &'static str {
    "native"
  }

  fn spawn(&self, ctx: ExecutorContext) -> CoreResult<Box<dyn InstanceHandle>> {
    let args = ctx.args.clone();
    let mut envs = ctx.envs.clone();
    let branch_key = ctx.branch_ctx.and_then(|c| c.key.as_ref());
    namespaces::validate_namespaces(ctx.isolation.namespaces.as_ref())?;
    let pre_exec_fds = ctx
      .sockets_map
      .get(&rslvns!(snorm ctx.registry_key).to_ustr())
      .map(|s| s.fds.clone())
      .unwrap_or_default();

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

    let mut working_dir = ctx.service.metadata.working_dir.clone();
    let mut uid_gid = None;
    if let Some((uid, gid, home, username)) = user_info {
      uid_gid = Some((uid, gid));
      if let Some(dir) = &working_dir
        && dir.as_str().starts_with("~")
      {
        working_dir = Some(Ustr::from(format!("{}{}", home, &dir.as_str()[1..])));
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
      envs.insert(Ustr::from("RIND_BRANCH_KEY"), key.clone());
    }

    if namespaces::needs_supervisor(&ctx.isolation) {
      let join_namespace_fds = namespaces::persisted_namespace_fds(ctx.isolation.scope.as_ref());
      return namespaces::spawn_supervised(
        ctx.run.exec.to_string(),
        args,
        envs,
        working_dir,
        uid_gid,
        pre_exec_fds,
        ctx.isolation,
        ctx.cgroup_path,
        join_namespace_fds,
        ctx.namespace_mounts.clone(),
        ctx.namespace_networks.clone(),
      );
    }

    let mut cmd = Command::new(ctx.run.exec.as_str());
    let namespaces = ctx.isolation.namespaces.clone();

    cmd
      .args(args.iter().map(|a| a.as_str()))
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped());

    if let Some(dir) = &working_dir {
      cmd.current_dir(dir.as_str());
    }

    if let Some((uid, gid)) = uid_gid {
      cmd.uid(uid);
      cmd.gid(gid);
    }

    unsafe {
      cmd.pre_exec(move || {
        libc::setsid();
        namespaces::apply_namespace_setup(namespaces.as_ref())?;
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
        Ok(Void)
      });
    }

    if !envs.is_empty() {
      cmd.envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let child = cmd.spawn()?;
    Ok(Box::new(ProcessHandle(child)))
  }
}
