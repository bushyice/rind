use crate::executors::{InstanceHandle, NamespaceNetworkConfig, SupervisorHandle};
use crate::services::{
  CapabilityPolicy, SeccompPolicy, ServiceCgroup, ServiceIsolation, ServiceNamespaces,
};
use rind_core::prelude::*;
use rind_primitives::mounts::{NamespaceMountEntry, mount_all_in_namespace};
use rind_primitives::utils::networking::setup_namespace_loopback;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::Mutex;

static PERSISTED_NAMESPACES: LazyLock<Mutex<HashMap<Ustr, Vec<File>>>> =
  LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn needs_supervisor(isolation: &ServiceIsolation) -> bool {
  isolation.needs_namespace_supervisor()
}

pub fn namespace_unshare_flags(ns: &ServiceNamespaces) -> libc::c_int {
  let mut flags = 0;
  if ns.mount || ns.mount_private || ns.rootfs.is_some() {
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
  if ns.pid {
    flags |= libc::CLONE_NEWPID;
  }
  flags
}

pub fn validate_namespaces(ns: Option<&ServiceNamespaces>) -> CoreResult<Void> {
  let _ = ns;
  Ok(Void)
}

pub unsafe fn apply_namespace_setup(ns: Option<&ServiceNamespaces>) -> std::io::Result<()> {
  let Some(ns) = ns else {
    return Ok(());
  };

  let ns_flags = namespace_unshare_flags(ns);
  if ns_flags != 0 && unsafe { libc::unshare(ns_flags) } < 0 {
    return Err(std::io::Error::last_os_error());
  }

  if ns.mount || ns.mount_private || ns.rootfs.is_some() {
    let target = CString::new("/").expect("literal path");
    if unsafe {
      libc::mount(
        std::ptr::null(),
        target.as_ptr(),
        std::ptr::null(),
        (libc::MS_REC | libc::MS_PRIVATE) as libc::c_ulong,
        std::ptr::null(),
      )
    } < 0
    {
      return Err(std::io::Error::last_os_error());
    }
  }

  if let Some(hostname) = &ns.hostname {
    let bytes = hostname.as_str().as_bytes();
    if unsafe { libc::sethostname(bytes.as_ptr() as *const libc::c_char, bytes.len()) } < 0 {
      return Err(std::io::Error::last_os_error());
    }
  }

  if let Some(rootfs) = &ns.rootfs {
    let root = CString::new(rootfs.as_str())
      .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let slash = CString::new("/").expect("literal path");
    if unsafe { libc::chroot(root.as_ptr()) } < 0 {
      return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::chdir(slash.as_ptr()) } < 0 {
      return Err(std::io::Error::last_os_error());
    }
  }

  Ok(())
}

fn pipe() -> CoreResult<(RawFd, RawFd)> {
  let mut fds = [0; 2];
  if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
    return Err(std::io::Error::last_os_error().into());
  }
  Ok((fds[0], fds[1]))
}

fn write_all_fd(fd: RawFd, bytes: &[u8]) -> std::io::Result<()> {
  let mut written = 0;
  while written < bytes.len() {
    let n = unsafe {
      libc::write(
        fd,
        bytes[written..].as_ptr() as *const libc::c_void,
        bytes.len() - written,
      )
    };
    if n < 0 {
      return Err(std::io::Error::last_os_error());
    }
    written += n as usize;
  }
  Ok(())
}

fn read_exact_fd(fd: RawFd, buf: &mut [u8]) -> std::io::Result<()> {
  let mut read = 0;
  while read < buf.len() {
    let n = unsafe {
      libc::read(
        fd,
        buf[read..].as_mut_ptr() as *mut libc::c_void,
        buf.len() - read,
      )
    };
    if n < 0 {
      return Err(std::io::Error::last_os_error());
    }
    if n == 0 {
      return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    }
    read += n as usize;
  }
  Ok(())
}

fn close_fd(fd: RawFd) {
  unsafe {
    libc::close(fd);
  }
}

pub fn write_user_maps(pid: u32, uid: u32, gid: u32) -> CoreResult<Void> {
  let proc = format!("/proc/{pid}");
  let _ = std::fs::write(format!("{proc}/setgroups"), "deny\n");
  std::fs::write(format!("{proc}/uid_map"), format!("0 {uid} 1\n"))?;
  std::fs::write(format!("{proc}/gid_map"), format!("0 {gid} 1\n"))?;
  Ok(Void)
}

pub fn apply_cgroup(
  path: Option<&std::path::Path>,
  cgroup: Option<&ServiceCgroup>,
  pid: u32,
) -> CoreResult<Void> {
  let Some(path) = path else {
    return Ok(Void);
  };
  std::fs::create_dir_all(path)?;
  if let Some(cgroup) = cgroup {
    if let Some(mem) = &cgroup.memory_max {
      let _ = std::fs::write(path.join("memory.max"), mem.as_str());
    }
    if let Some(cpu) = &cgroup.cpu_max {
      let _ = std::fs::write(path.join("cpu.max"), cpu.as_str());
    }
    if let Some(pids) = &cgroup.pids_max {
      let _ = std::fs::write(path.join("pids.max"), pids.as_str());
    }
  }
  std::fs::write(path.join("cgroup.procs"), pid.to_string())?;
  Ok(Void)
}

pub fn cap_number(name: &str) -> Option<i32> {
  match name.trim().to_ascii_lowercase().as_str() {
    "chown" => Some(0),
    "dac_override" => Some(1),
    "dac_read_search" => Some(2),
    "fowner" => Some(3),
    "fsetid" => Some(4),
    "kill" => Some(5),
    "setgid" => Some(6),
    "setuid" => Some(7),
    "setpcap" => Some(8),
    "linux_immutable" => Some(9),
    "net_bind_service" => Some(10),
    "net_broadcast" => Some(11),
    "net_admin" => Some(12),
    "net_raw" => Some(13),
    "ipc_lock" => Some(14),
    "ipc_owner" => Some(15),
    "sys_module" => Some(16),
    "sys_rawio" => Some(17),
    "sys_chroot" => Some(18),
    "sys_ptrace" => Some(19),
    "sys_pacct" => Some(20),
    "sys_admin" => Some(21),
    "sys_boot" => Some(22),
    "sys_nice" => Some(23),
    "sys_resource" => Some(24),
    "sys_time" => Some(25),
    "sys_tty_config" => Some(26),
    "mknod" => Some(27),
    "lease" => Some(28),
    "audit_write" => Some(29),
    "audit_control" => Some(30),
    "setfcap" => Some(31),
    "mac_override" => Some(32),
    "mac_admin" => Some(33),
    "syslog" => Some(34),
    "wake_alarm" => Some(35),
    "block_suspend" => Some(36),
    "audit_read" => Some(37),
    "perfmon" => Some(38),
    "bpf" => Some(39),
    "checkpoint_restore" => Some(40),
    _ => None,
  }
}

pub fn cap_numbers(names: &[Ustr]) -> Vec<i32> {
  names
    .iter()
    .filter_map(|cap| cap_number(cap.as_str()))
    .collect()
}

pub fn drop_bounding_set(caps: Option<&CapabilityPolicy>) -> std::io::Result<()> {
  let Some(caps) = caps else {
    return Ok(());
  };

  let keep: HashSet<i32> = cap_numbers(&caps.keep).into_iter().collect();

  if caps.drop.iter().any(|cap| cap.as_str() == "all") {
    for cap in 0..=40 {
      if keep.contains(&cap) {
        continue;
      }
      unsafe {
        libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0);
      }
    }
    return Ok(());
  }

  for cap in &caps.drop {
    let Some(cap) = cap_number(cap.as_str()) else {
      return Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("unknown capability '{}'", cap),
      ));
    };
    if unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) } < 0 {
      return Err(std::io::Error::last_os_error());
    }
  }
  Ok(())
}

pub fn apply_seccomp(seccomp: Option<&SeccompPolicy>) -> std::io::Result<()> {
  let Some(seccomp) = seccomp else {
    return Ok(());
  };
  if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
    return Err(std::io::Error::last_os_error());
  }
  match seccomp.profile.as_ref().map(|p| p.as_str()) {
    Some("strict") => {
      if unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_STRICT, 0, 0, 0) } < 0 {
        return Err(std::io::Error::last_os_error());
      }
      Ok(())
    }
    Some(profile) => Err(std::io::Error::new(
      std::io::ErrorKind::InvalidInput,
      format!("unsupported seccomp profile '{profile}'"),
    )),
    None if seccomp.path.is_some() => Err(std::io::Error::new(
      std::io::ErrorKind::InvalidInput,
      "seccomp filter file loading is not implemented",
    )),
    None => Ok(()),
  }
}

pub fn open_namespace_fds(pid: u32, ns: Option<&ServiceNamespaces>) -> Vec<File> {
  let Some(ns) = ns else {
    return Vec::new();
  };
  if !ns.persist {
    return Vec::new();
  }

  let mut names = Vec::new();
  if ns.mount || ns.mount_private || ns.rootfs.is_some() {
    names.push("mnt");
  }
  if ns.uts || ns.hostname.is_some() {
    names.push("uts");
  }
  if ns.ipc {
    names.push("ipc");
  }
  if ns.net {
    names.push("net");
  }
  if ns.user {
    names.push("user");
  }
  if ns.cgroup {
    names.push("cgroup");
  }
  if ns.pid {
    names.push("pid_for_children");
  }

  names
    .into_iter()
    .filter_map(|name| File::open(format!("/proc/{pid}/ns/{name}")).ok())
    .collect()
}

pub fn persisted_namespace_fds(scope: Option<&Ustr>) -> Vec<File> {
  let Some(scope) = scope else {
    return Vec::new();
  };
  let store = PERSISTED_NAMESPACES
    .lock()
    .expect("persisted namespace store lock poisoned");
  store
    .get(scope)
    .map(|fds| fds.iter().filter_map(|fd| fd.try_clone().ok()).collect())
    .unwrap_or_default()
}

pub fn store_persisted_namespaces(scope: Option<&Ustr>, fds: &[File]) {
  let Some(scope) = scope else {
    return;
  };
  if fds.is_empty() {
    return;
  }
  let mut store = PERSISTED_NAMESPACES
    .lock()
    .expect("persisted namespace store lock poisoned");
  store.insert(
    scope.clone(),
    fds.iter().filter_map(|fd| fd.try_clone().ok()).collect(),
  );
}

pub fn join_ns_fds(fds: &[File]) -> std::io::Result<()> {
  for fd in fds {
    if unsafe { libc::setns(fd.as_raw_fd(), 0) } < 0 {
      return Err(std::io::Error::last_os_error());
    }
  }
  Ok(())
}

fn exec_service(
  exec: &str,
  args: &[Ustr],
  envs: &HashMap<Ustr, Ustr>,
  cwd: Option<&Ustr>,
  uid_gid: Option<(u32, u32)>,
  pre_exec_fds: &[RawFd],
  isolation: &ServiceIsolation,
) -> ! {
  if let Some(dir) = cwd {
    let _ = std::env::set_current_dir(dir.as_str());
  }

  for (idx, fd) in pre_exec_fds.iter().enumerate() {
    let target_fd = (3 + idx) as RawFd;
    if *fd != target_fd {
      unsafe {
        libc::dup2(*fd, target_fd);
      }
    }
    unsafe {
      let flags = libc::fcntl(target_fd, libc::F_GETFD);
      if flags >= 0 {
        let _ = libc::fcntl(target_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
      }
    }
  }

  if drop_bounding_set(isolation.capabilities.as_ref()).is_err() {
    unsafe { libc::_exit(126) }
  }

  if let Some((uid, gid)) = uid_gid {
    unsafe {
      if libc::setgid(gid) < 0 {
        libc::_exit(126);
      }
      if libc::setuid(uid) < 0 {
        libc::_exit(126);
      }
    }
  }

  if apply_seccomp(isolation.seccomp.as_ref()).is_err() {
    unsafe {
      libc::_exit(126);
    }
  }

  let c_exec = CString::new(exec).unwrap_or_else(|_| CString::new("/bin/false").unwrap());
  let mut c_args = Vec::with_capacity(args.len() + 1);
  c_args.push(c_exec.clone());
  c_args.extend(args.iter().filter_map(|a| CString::new(a.as_str()).ok()));
  let mut argv = c_args.iter().map(|a| a.as_ptr()).collect::<Vec<_>>();
  argv.push(std::ptr::null());

  let c_envs = envs
    .iter()
    .filter_map(|(k, v)| CString::new(format!("{}={}", k.as_str(), v.as_str())).ok())
    .collect::<Vec<_>>();
  let mut envp = c_envs.iter().map(|e| e.as_ptr()).collect::<Vec<_>>();
  envp.push(std::ptr::null());

  unsafe {
    libc::execve(c_exec.as_ptr(), argv.as_ptr(), envp.as_ptr());
    libc::_exit(127);
  }
}

fn apply_namespace_network(networks: &[NamespaceNetworkConfig]) {
  let _ = networks;
  setup_namespace_loopback();
}

fn wait_and_exit(pid: libc::pid_t) -> ! {
  let mut status = 0;
  loop {
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    if waited < 0 {
      let err = std::io::Error::last_os_error();
      if err.raw_os_error() == Some(libc::EINTR) {
        continue;
      }
      unsafe {
        libc::_exit(127);
      }
    }
    break;
  }

  unsafe {
    if libc::WIFEXITED(status) {
      libc::_exit(libc::WEXITSTATUS(status));
    }
    if libc::WIFSIGNALED(status) {
      libc::_exit(128 + libc::WTERMSIG(status));
    }
    libc::_exit(127);
  }
}

fn run_pid_init(
  exec: String,
  args: Vec<Ustr>,
  envs: HashMap<Ustr, Ustr>,
  cwd: Option<Ustr>,
  uid_gid: Option<(u32, u32)>,
  pre_exec_fds: Vec<RawFd>,
  isolation: ServiceIsolation,
) -> ! {
  let child = unsafe { libc::fork() };
  if child < 0 {
    unsafe {
      libc::_exit(127);
    }
  }
  if child == 0 {
    exec_service(
      &exec,
      &args,
      &envs,
      cwd.as_ref(),
      uid_gid,
      &pre_exec_fds,
      &isolation,
    );
  }

  let mut main_exited = None;
  loop {
    let mut status = 0;
    let waited = unsafe { libc::waitpid(-1, &mut status, 0) };
    if waited < 0 {
      let err = std::io::Error::last_os_error();
      if err.raw_os_error() == Some(libc::EINTR) {
        continue;
      }
      break;
    }
    if waited == child {
      main_exited = Some(status);
    }
    if main_exited.is_some() {
      while unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) } > 0 {}
      break;
    }
  }

  let status = main_exited.unwrap_or(127 << 8);
  unsafe {
    if libc::WIFEXITED(status) {
      libc::_exit(libc::WEXITSTATUS(status));
    }
    if libc::WIFSIGNALED(status) {
      libc::_exit(128 + libc::WTERMSIG(status));
    }
    libc::_exit(127);
  }
}

pub fn spawn_supervised(
  exec: String,
  args: Vec<Ustr>,
  envs: HashMap<Ustr, Ustr>,
  cwd: Option<Ustr>,
  uid_gid: Option<(u32, u32)>,
  pre_exec_fds: Vec<RawFd>,
  isolation: ServiceIsolation,
  cgroup_path: Option<PathBuf>,
  join_namespace_fds: Vec<File>,
  namespace_mounts: Vec<NamespaceMountEntry>,
  namespace_networks: Vec<NamespaceNetworkConfig>,
) -> CoreResult<Box<dyn InstanceHandle>> {
  let (stdin_r, stdin_w) = pipe()?;
  let (stdout_r, stdout_w) = pipe()?;
  let (stderr_r, stderr_w) = pipe()?;
  let (ready_r, ready_w) = pipe()?;
  let (go_r, go_w) = pipe()?;

  let pid = unsafe { libc::fork() };
  if pid < 0 {
    return Err(std::io::Error::last_os_error().into());
  }

  if pid == 0 {
    close_fd(stdin_w);
    close_fd(stdout_r);
    close_fd(stderr_r);
    close_fd(ready_r);
    close_fd(go_w);

    unsafe {
      libc::setsid();
      libc::dup2(stdin_r, libc::STDIN_FILENO);
      libc::dup2(stdout_w, libc::STDOUT_FILENO);
      libc::dup2(stderr_w, libc::STDERR_FILENO);
    }

    let ns = isolation.namespaces.clone().unwrap_or_default();
    if !join_namespace_fds.is_empty() {
      if join_ns_fds(&join_namespace_fds).is_err() {
        unsafe {
          libc::_exit(125);
        }
      }
    } else if ns.user {
      if unsafe { libc::unshare(libc::CLONE_NEWUSER) } < 0 {
        unsafe {
          libc::_exit(125);
        }
      }
      let _ = write_all_fd(ready_w, b"U");
      let mut buf = [0; 1];
      if read_exact_fd(go_r, &mut buf).is_err() || buf[0] != b'M' {
        unsafe {
          libc::_exit(125);
        }
      }
    }

    if join_namespace_fds.is_empty() && unsafe { apply_namespace_setup(Some(&ns)) }.is_err() {
      unsafe {
        libc::_exit(125);
      }
    }

    if ns.net || ns.mount || ns.mount_private || ns.rootfs.is_some() {
      if ns.net {
        apply_namespace_network(&namespace_networks);
      }
      if !namespace_mounts.is_empty() {
        if mount_all_in_namespace(&namespace_mounts).is_err() {
          unsafe { libc::_exit(125) }
        }
      }
    }

    let _ = write_all_fd(ready_w, b"R");
    let mut buf = [0; 1];
    if read_exact_fd(go_r, &mut buf).is_err() || buf[0] != b'G' {
      unsafe {
        libc::_exit(125);
      }
    }

    let service_uid_gid = if ns.user { Some((0, 0)) } else { uid_gid };

    if ns.pid {
      let init_pid = unsafe { libc::fork() };
      if init_pid < 0 {
        unsafe {
          libc::_exit(127);
        }
      }
      if init_pid == 0 {
        if ns.init {
          run_pid_init(
            exec,
            args,
            envs,
            cwd,
            service_uid_gid,
            pre_exec_fds,
            isolation,
          );
        } else {
          exec_service(
            &exec,
            &args,
            &envs,
            cwd.as_ref(),
            service_uid_gid,
            &pre_exec_fds,
            &isolation,
          );
        }
      }
      wait_and_exit(init_pid);
    }

    exec_service(
      &exec,
      &args,
      &envs,
      cwd.as_ref(),
      service_uid_gid,
      &pre_exec_fds,
      &isolation,
    );
  }

  close_fd(stdin_r);
  close_fd(stdout_w);
  close_fd(stderr_w);
  close_fd(ready_w);
  close_fd(go_r);

  let supervisor_pid = pid as u32;
  let mut phase = [0; 1];
  read_exact_fd(ready_r, &mut phase)?;
  if phase[0] == b'U' {
    let (uid, gid) = uid_gid.ok_or_else(|| {
      CoreError::InvalidState("user namespace needs a resolved scope or service user".to_string())
    })?;
    write_user_maps(supervisor_pid, uid, gid)?;
    write_all_fd(go_w, b"M")?;
    read_exact_fd(ready_r, &mut phase)?;
  }
  if phase[0] != b'R' {
    return Err(CoreError::InvalidState(
      "namespace supervisor did not reach ready state".to_string(),
    ));
  }

  apply_cgroup(
    cgroup_path.as_deref(),
    isolation.cgroup.as_ref(),
    supervisor_pid,
  )?;
  let namespace_fds = if join_namespace_fds.is_empty() {
    open_namespace_fds(supervisor_pid, isolation.namespaces.as_ref())
  } else {
    join_namespace_fds
      .iter()
      .filter_map(|fd| fd.try_clone().ok())
      .collect()
  };
  store_persisted_namespaces(isolation.scope.as_ref(), &namespace_fds);
  write_all_fd(go_w, b"G")?;

  close_fd(ready_r);
  close_fd(go_w);

  let handle = SupervisorHandle {
    pid: supervisor_pid,
    stdout: Some(unsafe { File::from_raw_fd(stdout_r) }),
    stderr: Some(unsafe { File::from_raw_fd(stderr_r) }),
    stdin: Some(unsafe { File::from_raw_fd(stdin_w) }),
    _namespace_fds: namespace_fds,
  };

  Ok(Box::new(handle))
}
