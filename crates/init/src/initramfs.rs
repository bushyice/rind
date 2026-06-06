use std::ffi::CString;
use std::fs;

use rind_core::types::Void;

use crate::early;

fn root_fs_type() -> Option<String> {
  let mounts = fs::read_to_string("/proc/self/mounts").ok()?;
  mounts.lines().find_map(|line| {
    let mut parts = line.split_whitespace();
    let _source = parts.next()?;
    let target = parts.next()?;
    let fstype = parts.next()?;
    if target == "/" {
      Some(fstype.to_string())
    } else {
      None
    }
  })
}

pub fn should_run_initramfs() -> bool {
  if let Ok(v) = std::env::var("RIND_INITRAMFS") {
    match v.as_str() {
      "1" | "true" | "TRUE" | "yes" | "YES" => return true,
      "0" | "false" | "FALSE" | "no" | "NO" => return false,
      _ => {}
    }
  }

  if std::path::Path::new("/etc/initramfs-release").exists() {
    return true;
  }

  if let Some(fs_type) = root_fs_type() {
    return fs_type == "rootfs" || fs_type == "tmpfs";
  }

  false
}

fn parse_kernel_cmdline() -> String {
  fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

fn resolve_root_device(spec: &str) -> Result<String, String> {
  if let Some(uuid) = spec.strip_prefix("UUID=") {
    let uuid_path = format!("/dev/disk/by-uuid/{uuid}");
    if std::path::Path::new(&uuid_path).exists() {
      return Ok(uuid_path);
    }

    for entry in fs::read_dir("/dev/disk/by-uuid")
      .map_err(|e| format!("failed to read /dev/disk/by-uuid: {e}"))?
    {
      let entry = entry.map_err(|e| format!("failed to read dir entry: {e}"))?;
      if entry
        .file_name()
        .to_string_lossy()
        .eq_ignore_ascii_case(uuid)
      {
        return Ok(entry.path().to_string_lossy().into_owned());
      }
    }
    return Err(format!("no device found for UUID={uuid}"));
  }

  if let Some(label) = spec.strip_prefix("LABEL=") {
    let label_path = format!("/dev/disk/by-label/{label}");
    if std::path::Path::new(&label_path).exists() {
      return Ok(label_path);
    }

    for entry in fs::read_dir("/dev/disk/by-label")
      .map_err(|e| format!("failed to read /dev/disk/by-label: {e}"))?
    {
      let entry = entry.map_err(|e| format!("failed to read dir entry: {e}"))?;
      if entry
        .file_name()
        .to_string_lossy()
        .eq_ignore_ascii_case(label)
      {
        return Ok(entry.path().to_string_lossy().into_owned());
      }
    }
    return Err(format!("no device found for LABEL={label}"));
  }

  if spec.starts_with('/') {
    if std::path::Path::new(spec).exists() {
      return Ok(spec.to_string());
    }
    return Err(format!("root device {spec} does not exist"));
  }

  let dev_path = format!("/dev/{spec}");
  if std::path::Path::new(&dev_path).exists() {
    return Ok(dev_path);
  }

  Err(format!("cannot resolve root device: {spec}"))
}

fn detect_root_device() -> Result<String, String> {
  if let Ok(spec) = std::env::var("RIND_ROOT") {
    return resolve_root_device(&spec);
  }

  let cmdline = parse_kernel_cmdline();
  for token in cmdline.split_whitespace() {
    if let Some(spec) = token.strip_prefix("root=") {
      return resolve_root_device(spec);
    }
  }

  Err("no root= found in kernel cmdline and RIND_ROOT not set".to_string())
}

fn detect_root_fstype(device: &str) -> Option<String> {
  let output = std::process::Command::new("blkid")
    .args(["-o", "value", "-s", "TYPE", device])
    .output()
    .ok()?;
  if output.status.success() {
    let fs_type = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !fs_type.is_empty() {
      return Some(fs_type);
    }
  }

  let mounts = fs::read_to_string("/proc/self/mounts").ok()?;
  mounts.lines().find_map(|line| {
    let mut parts = line.split_whitespace();
    let source = parts.next()?;
    let _target = parts.next()?;
    let fstype = parts.next()?;
    if source == device {
      Some(fstype.to_string())
    } else {
      None
    }
  })
}

fn mount_real_root(device: &str) -> Result<String, String> {
  let new_root = "/new_root";

  if !std::path::Path::new(new_root).exists() {
    fs::create_dir_all(new_root).map_err(|e| format!("failed to create {new_root}: {e}"))?;
  }

  if early::is_mounted(new_root) {
    return Ok(new_root.to_string());
  }

  let fstype = detect_root_fstype(device).unwrap_or_else(|| {
    eprintln!("[initrd] could not detect root fs type, assuming ext4");
    "ext4".to_string()
  });

  eprintln!("[initrd] mounting root device {device} ({fstype}) on {new_root}");

  let c_device = CString::new(device).unwrap();
  let c_target = CString::new(new_root).unwrap();
  let c_fstype = CString::new(fstype.as_str()).unwrap();

  let rc = unsafe {
    libc::mount(
      c_device.as_ptr(),
      c_target.as_ptr(),
      c_fstype.as_ptr(),
      0,
      std::ptr::null(),
    )
  };

  if rc < 0 {
    let err = std::io::Error::last_os_error();
    return Err(format!(
      "failed to mount root {device} on {new_root}: {err}"
    ));
  }

  Ok(new_root.to_string())
}

fn move_mounts_to_new_root(new_root: &str) -> Result<(), String> {
  let mounts_to_move: &[&str] = &["", "/dev", "/proc", "/sys", "/run", "/tmp"];

  for mount_point in mounts_to_move {
    let source = if mount_point.is_empty() {
      "/".to_string()
    } else {
      mount_point.to_string()
    };

    let dest = format!("{new_root}{source}");

    if !std::path::Path::new(&dest).exists() {
      let _ = fs::create_dir_all(&dest);
    }

    if early::is_mounted(&source) && source != "/" {
      let c_source = CString::new(source.as_str()).unwrap();
      let c_dest = CString::new(dest.as_str()).unwrap();

      let rc = unsafe {
        libc::mount(
          c_source.as_ptr(),
          c_dest.as_ptr(),
          std::ptr::null(),
          libc::MS_MOVE,
          std::ptr::null(),
        )
      };

      if rc < 0 {
        let err = std::io::Error::last_os_error();
        eprintln!("[initrd] failed to move mount {source} to {dest}: {err}");
      }
    }
  }

  Ok(())
}

fn pivot_root(new_root: &str) -> Result<(), String> {
  let put_old = format!("{new_root}/old_root");

  if !std::path::Path::new(&put_old).exists() {
    fs::create_dir_all(&put_old)
      .map_err(|e| format!("failed to create put_old dir {put_old}: {e}"))?;
  }

  let c_new_root = CString::new(new_root).unwrap();
  let c_put_old = CString::new(put_old.as_str()).unwrap();

  let rc = unsafe {
    nix::libc::syscall(
      nix::libc::SYS_pivot_root,
      c_new_root.as_ptr(),
      c_put_old.as_ptr(),
    )
  };

  if rc < 0 {
    let err = std::io::Error::last_os_error();
    return Err(format!("pivot_root failed: {err}"));
  }

  Ok(())
}

fn umount_old_root() {
  let old_root = "/old_root";

  if !std::path::Path::new(old_root).exists() {
    return;
  }

  let c_old_root = CString::new(old_root).unwrap();

  let rc = unsafe { libc::umount2(c_old_root.as_ptr(), libc::MNT_DETACH) };

  if rc < 0 {
    eprintln!(
      "[initrd] failed to unmount old root: {err}",
      err = std::io::Error::last_os_error()
    );
  } else {
    let _ = fs::remove_dir(old_root);
  }
}

pub fn run_initramfs_boot() -> Result<bool, Box<dyn std::error::Error>> {
  eprintln!("[initrd] starting initramfs boot");

  early::mount_essential_filesystems().unwrap_or_else(|e| {
    eprintln!("[initrd] warning: {e}");
  });
  early::create_device_nodes().unwrap_or_else(|e| {
    eprintln!("[initrd] warning: {e}");
  });

  let device = match detect_root_device() {
    Ok(d) => d,
    Err(e) => {
      eprintln!("[initrd] {e}, falling back to normal init flow");
      return Ok(true);
    }
  };

  let new_root = match mount_real_root(&device) {
    Ok(path) => path,
    Err(e) => {
      eprintln!("[initrd] {e}, falling back to normal init flow");
      return Ok(true);
    }
  };

  if let Err(e) = move_mounts_to_new_root(&new_root) {
    eprintln!("[initrd] warning during mount move: {e}");
  }

  if let Err(e) = pivot_root(&new_root) {
    eprintln!("[initrd] {e}, falling back to normal init flow");
    return Ok(true);
  }

  umount_old_root();

  eprintln!("[initrd] pivot_root succeeded, execing into real root");

  exec_real_init_from_env()?;
  Ok(false)
}

pub fn initramfs_init() -> Result<bool, Box<dyn std::error::Error>> {
  run_initramfs_boot()
}

pub fn exec_real_init_from_env() -> Result<Void, Box<dyn std::error::Error>> {
  use std::os::unix::ffi::OsStrExt;
  let init_path = std::env::var("RIND_REAL_INIT").unwrap_or("/usr/bin/init".to_string());
  let path = std::path::Path::new(&init_path);
  let c_path = CString::new(path.as_os_str().as_bytes())?;

  let mut argv = Vec::new();
  argv.push(c_path.clone());
  if let Ok(extra) = std::env::var("RIND_REAL_INIT_ARGS") {
    for arg in extra.split_whitespace() {
      argv.push(CString::new(arg)?);
    }
  }

  nix::unistd::execv(&c_path, &argv)?;
  Ok(Void)
}
