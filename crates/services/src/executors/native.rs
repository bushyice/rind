use crate::executors::{Executor, ExecutorContext, InstanceHandle, ProcessHandle};
use crate::{ServiceType, namespaces};
use rind_core::prelude::*;
use rind_core::utils::read_env_file;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::{Duration, Instant};

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
        pre_exec_fds.into_iter().collect::<Vec<_>>(),
        ctx.isolation,
        ctx.cgroup_path,
        join_namespace_fds,
        ctx.namespace_mounts.clone(),
        ctx.namespace_networks.clone(),
      );
    }

    if let Some(files) = &ctx.run.files {
      for file in files {
        if !file.clean && !file.create {
          continue;
        }

        let path = PathBuf::from(&file.path);

        if file.clean && path.exists() && (!file.once || ctx.service.instances.len() < 1) {
          if file.dir {
            std::fs::remove_dir_all(&path)?;
          } else {
            std::fs::remove_file(&path)?;
          }
        }

        if file.create && !path.exists() {
          if file.dir {
            std::fs::create_dir_all(&path)?;
          } else {
            if let Some(parent) = path.parent() {
              std::fs::create_dir_all(parent)?;
            }
            let content = file.content.clone().unwrap_or_default();
            std::fs::write(&path, content)?;
          }
        }
      }
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

    // if !envs.is_empty() {
    //   cmd.envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    // }

    unsafe {
      cmd.pre_exec(move || {
        libc::setsid();
        namespaces::apply_namespace_setup(namespaces.as_ref())?;

        for (k, v) in &envs {
          let ck = std::ffi::CString::new(k.as_str()).unwrap();
          let cv = std::ffi::CString::new(v.as_str()).unwrap();
          if libc::setenv(ck.as_ptr(), cv.as_ptr(), 1) != 0 {
            return Err(std::io::Error::last_os_error());
          }
        }

        if envs.contains_key("LISTEN_FDS") {
          let pid_cstr = std::ffi::CString::new(libc::getpid().to_string()).unwrap();
          let listen_pid_key = std::ffi::CString::new("LISTEN_PID").unwrap();
          if libc::setenv(listen_pid_key.as_ptr(), pid_cstr.as_ptr(), 1) != 0 {
            return Err(std::io::Error::last_os_error());
          }
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
        Ok(Void)
      });
    }

    let mut child = cmd.spawn()?;

    if ctx.service.metadata.r#type == ServiceType::Wait {
      let timeout = Duration::from_secs(
        ctx
          .service
          .metadata
          .options
          .as_ref()
          .and_then(|x| x.get("timeout").cloned())
          .or_else(|| std::env::var("RIND_SERVICE_TIMEOUT").ok())
          .and_then(|x| x.parse::<u64>().ok())
          .unwrap_or(30),
      );
      let start = Instant::now();
      loop {
        match child.try_wait() {
          Ok(Some(_)) => break,
          Ok(None) => {
            if start.elapsed() >= timeout {
              child.kill()?;
              child.wait()?;
              return Err(CoreError::custom(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                  "Wait service '{}' timed out after {}s",
                  ctx.service.metadata.name,
                  timeout.as_secs()
                ),
              )));
            }
            std::thread::sleep(Duration::from_millis(50));
          }
          Err(e) => {
            return Err(CoreError::custom(std::io::Error::new(
              std::io::ErrorKind::Other,
              format!("Wait service '{}' error: {e}", ctx.service.metadata.name),
            )));
          }
        }
      }
    }

    Ok(Box::new(ProcessHandle(child)))
  }
}
