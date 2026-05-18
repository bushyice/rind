use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::{collections::HashMap, ffi::CString};

use rind_core::{
  boot::BootEngine,
  error::CoreResult,
  prelude::{LogHandle, MetadataRegistry, Resources},
};
use rind_plugins::{PluginCapability, collect_plugins};

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

fn should_short_circuit() -> bool {
  std::env::var("RIND_INITRAMFS_SHORT_CIRCUIT")
    .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
    .unwrap_or(false)
}

pub fn initramfs_init() -> CoreResult<bool> {
  let log = LogHandle::mock();
  let mut boot = BootEngine::default();
  let init_plugins_path = if let Ok(path) = std::env::var("RIND_INIT_PLUGINS_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/lib/rind/plugins/initramfs")
  };

  let plugins = collect_plugins(
    init_plugins_path,
    &log,
    Some(PluginCapability::ORCHESTRATORS | PluginCapability::INITRD),
  )?
  .collect::<Vec<_>>();

  for plugin in plugins.iter() {
    boot.orchestrators.extend(
      plugin
        .provide_orchestrators()
        .into_iter()
        .filter(|x| {
          x.when()
            .cycle
            .contains(&rind_core::prelude::BootCycle::PreBoot)
        })
        .collect(),
    );
  }

  boot.pre_boot(
    &mut MetadataRegistry::default(),
    &mut HashMap::default(),
    &mut Resources::default(),
    log,
  )?;

  for plugin in plugins {
    drop(plugin);
  }

  Ok(!should_short_circuit())
}

pub fn exec_real_init_from_env() -> Result<(), Box<dyn std::error::Error>> {
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
  Ok(())
}
