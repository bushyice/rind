use std::ffi::CString;
use std::fs;
use std::path::Path;

use nix::mount::{MsFlags, mount};
use nix::sys::stat::{Mode, SFlag, mknod};

use crate::fstab;

pub fn mount_essential_filesystems() -> Result<(), String> {
  let essentials: &[(&str, &str, &str, MsFlags)] = &[
    ("/dev", "devtmpfs", "devtmpfs", MsFlags::MS_NOSUID),
    (
      "/proc",
      "proc",
      "proc",
      MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
    ),
    (
      "/sys",
      "sysfs",
      "sysfs",
      MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
    ),
    (
      "/tmp",
      "tmpfs",
      "tmpfs",
      MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
    ),
    (
      "/run",
      "tmpfs",
      "tmpfs",
      MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
    ),
  ];

  for &(target, source, fstype, flags) in essentials {
    if is_mounted(target) {
      continue;
    }

    let _ = fs::create_dir_all(target);

    match mount(
      Some(source),
      target,
      Some(fstype),
      flags,
      Option::<&str>::None,
    ) {
      Ok(()) => eprintln!("[early] mounted {fstype} on {target}"),
      Err(e) => {
        eprintln!("[early] failed to mount {fstype} on {target}: {e}");
      }
    }
  }

  Ok(())
}

pub fn create_device_nodes() -> Result<(), String> {
  if is_mounted("/dev") {
    return Ok(());
  }

  let nodes: &[(&str, SFlag, Mode, u32, u32)] = &[
    (
      "/dev/null",
      SFlag::S_IFCHR,
      Mode::from_bits(0o666).unwrap(),
      1,
      3,
    ),
    (
      "/dev/zero",
      SFlag::S_IFCHR,
      Mode::from_bits(0o666).unwrap(),
      1,
      5,
    ),
    (
      "/dev/console",
      SFlag::S_IFCHR,
      Mode::from_bits(0o600).unwrap(),
      5,
      1,
    ),
    (
      "/dev/ptmx",
      SFlag::S_IFCHR,
      Mode::from_bits(0o666).unwrap(),
      5,
      2,
    ),
    (
      "/dev/tty",
      SFlag::S_IFCHR,
      Mode::from_bits(0o666).unwrap(),
      5,
      0,
    ),
    (
      "/dev/random",
      SFlag::S_IFCHR,
      Mode::from_bits(0o666).unwrap(),
      1,
      8,
    ),
    (
      "/dev/urandom",
      SFlag::S_IFCHR,
      Mode::from_bits(0o666).unwrap(),
      1,
      9,
    ),
  ];

  for &(path, flags, mode, major, minor) in nodes {
    if Path::new(path).exists() {
      continue;
    }
    match mknod(path, flags, mode, libc::makedev(major, minor)) {
      Ok(()) => eprintln!("[early] created device node {path}"),
      Err(e) => {
        eprintln!("[early] failed to create device node {path}: {e}");
      }
    }
  }

  Ok(())
}

pub fn set_hostname() -> Result<(), String> {
  let hostname_path = "/etc/hostname";
  if !Path::new(hostname_path).exists() {
    eprintln!("[early] no /etc/hostname found, skipping hostname setup");
    return Ok(());
  }

  let hostname =
    fs::read_to_string(hostname_path).map_err(|e| format!("failed to read /etc/hostname: {e}"))?;
  let hostname = hostname.trim();
  if hostname.is_empty() {
    return Ok(());
  }

  let c_hostname =
    CString::new(hostname).map_err(|e| format!("hostname contains null byte: {e}"))?;

  unsafe {
    let rc = libc::sethostname(c_hostname.as_ptr(), hostname.len());
    if rc != 0 {
      let err = std::io::Error::last_os_error();
      eprintln!("[early] failed to set hostname: {err}");
    } else {
      eprintln!("[early] hostname set to '{hostname}'");
    }
  }

  Ok(())
}

pub fn mount_fstab() -> Result<(), String> {
  let entries = fstab::parse().map_err(|e| format!("fstab: {e}"))?;

  for entry in &entries {
    if is_mounted(&entry.mountpoint) {
      let flags = fstdab_options_to_flags(&entry.options);
      let target = CString::new(entry.mountpoint.as_str())
        .map_err(|_| format!("invalid mountpoint: {}", entry.mountpoint))?;
      let rc = unsafe {
        libc::mount(
          std::ptr::null(),
          target.as_ptr(),
          std::ptr::null(),
          flags.bits() | libc::MS_REMOUNT,
          std::ptr::null(),
        )
      };
      if rc < 0 {
        let err = std::io::Error::last_os_error();
        eprintln!("[fstab] failed to remount {}: {err}", entry.mountpoint);
      } else {
        eprintln!("[fstab] remounted {} with fstab options", entry.mountpoint);
      }
      continue;
    }

    let device = match resolve_fstab_device(&entry.device) {
      Some(d) => d,
      None => {
        eprintln!(
          "[fstab] device not found: {}, skipping {}",
          entry.device, entry.mountpoint
        );
        continue;
      }
    };

    let flags = fstdab_options_to_flags(&entry.options);

    let _ = fs::create_dir_all(&entry.mountpoint);

    let source = CString::new(device.as_str()).ok();
    let target = CString::new(entry.mountpoint.as_str())
      .map_err(|_| format!("invalid mountpoint: {}", entry.mountpoint))?;
    let fstype = CString::new(entry.fstype.as_str()).ok();
    let data = mount_data_from_options(&entry.options);

    let data_ptr = data.as_ref().map(|s| s.as_ptr() as *const libc::c_void);

    let rc = unsafe {
      libc::mount(
        source.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
        target.as_ptr(),
        fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
        flags.bits(),
        data_ptr.unwrap_or(std::ptr::null()),
      )
    };

    if rc < 0 {
      let err = std::io::Error::last_os_error();
      eprintln!(
        "[fstab] failed to mount {} on {}: {err}",
        device, entry.mountpoint
      );
    } else {
      eprintln!(
        "[fstab] mounted {} on {} ({})",
        device, entry.mountpoint, entry.fstype
      );
    }
  }

  Ok(())
}

fn resolve_fstab_device(spec: &str) -> Option<String> {
  if let Some(val) = spec.strip_prefix("UUID=") {
    return find_disk_by_value("by-uuid", val);
  }
  if let Some(val) = spec.strip_prefix("LABEL=") {
    return find_disk_by_value("by-label", val);
  }
  if let Some(val) = spec.strip_prefix("PARTUUID=") {
    return find_disk_by_value("by-partuuid", val);
  }
  if let Some(val) = spec.strip_prefix("PARTLABEL=") {
    return find_disk_by_value("by-partlabel", val);
  }
  if spec.starts_with('/') {
    return Some(spec.to_string());
  }
  Some(format!("/dev/{spec}"))
}

fn find_disk_by_value(by_dir: &str, value: &str) -> Option<String> {
  let path = format!("/dev/disk/{by_dir}/{value}");
  if Path::new(&path).exists() {
    return Some(path);
  }
  let dir = fs::read_dir(format!("/dev/disk/{by_dir}")).ok()?;
  for entry in dir.flatten() {
    if entry
      .file_name()
      .to_string_lossy()
      .eq_ignore_ascii_case(value)
    {
      return Some(entry.path().to_string_lossy().into_owned());
    }
  }
  None
}

fn mount_data_from_options(options: &[String]) -> Option<CString> {
  let data_entries: Vec<&str> = options
    .iter()
    .filter(|o| {
      !matches!(
        o.as_str(),
        "defaults"
          | "noauto"
          | "noexec"
          | "nosuid"
          | "nodev"
          | "noatime"
          | "relatime"
          | "sync"
          | "async"
          | "ro"
          | "rw"
          | "nouser"
          | "user"
      )
    })
    .map(|s| s.as_str())
    .collect();

  if data_entries.is_empty() {
    None
  } else {
    CString::new(data_entries.join(",")).ok()
  }
}

fn fstdab_options_to_flags(options: &[String]) -> MsFlags {
  let mut flags = MsFlags::empty();
  for option in options {
    match option.as_str() {
      "defaults" => {}
      "noatime" => flags |= MsFlags::MS_NOATIME,
      "relatime" => flags |= MsFlags::MS_RELATIME,
      "noexec" => flags |= MsFlags::MS_NOEXEC,
      "nosuid" => flags |= MsFlags::MS_NOSUID,
      "nodev" => flags |= MsFlags::MS_NODEV,
      "sync" => flags |= MsFlags::MS_SYNCHRONOUS,
      "async" => {}
      "ro" => flags |= MsFlags::MS_RDONLY,
      "rw" => {}
      "noauto" => {}
      "nouser" => {}
      "user" => {}
      "bind" => flags |= MsFlags::MS_BIND,
      "rec" => flags |= MsFlags::MS_REC,
      "private" => flags |= MsFlags::MS_PRIVATE,
      "shared" => flags |= MsFlags::MS_SHARED,
      "slave" => flags |= MsFlags::MS_SLAVE,
      _ => {}
    }
  }
  flags
}

pub fn is_mounted(target: &str) -> bool {
  let Ok(content) = fs::read_to_string("/proc/self/mounts") else {
    return false;
  };
  content.lines().any(|line| {
    let mut parts = line.split_whitespace();
    let _ = parts.next();
    parts.next() == Some(target)
  })
}

pub fn load_env() {
  unsafe {
    for (key, value) in rind_core::utils::read_env_file("/etc/.env") {
      std::env::set_var(&key, &value);
    }
  }
}
