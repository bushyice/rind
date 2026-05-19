use std::fs;
use std::path::Path;

use nix::mount::{MntFlags, MsFlags, mount, umount2};
use nix::unistd::{chdir, pivot_root};
use rind_plugins::prelude::*;

#[derive(Default)]
struct InitramfsOrchestrator;

fn env_truthy(name: &str) -> bool {
  std::env::var(name)
    .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
    .unwrap_or(false)
}

fn ensure_dir(path: &str) -> CoreResult<()> {
  fs::create_dir_all(path)?;
  Ok(())
}

fn move_mount_if_exists(src: &str, new_root: &str) -> CoreResult<()> {
  if !Path::new(src).exists() {
    return Ok(());
  }
  let target = format!("{new_root}{}", src);
  ensure_dir(&target)?;
  mount(
    Some(src),
    target.as_str(),
    Option::<&str>::None,
    MsFlags::MS_MOVE,
    Option::<&str>::None,
  )?;
  Ok(())
}

fn switch_to_real_root() -> CoreResult<()> {
  let real_root = match std::env::var("RIND_INITRAMFS_REAL_ROOT") {
    Ok(v) if !v.trim().is_empty() => v,
    _ => return Ok(()),
  };

  let new_root = std::env::var("RIND_INITRAMFS_NEW_ROOT").unwrap_or("/newroot".to_string());
  let fs_type = std::env::var("RIND_INITRAMFS_REAL_ROOT_FSTYPE").ok();
  let mount_data = std::env::var("RIND_INITRAMFS_REAL_ROOT_DATA").ok();
  let readonly = env_truthy("RIND_INITRAMFS_REAL_ROOT_READONLY");

  ensure_dir(&new_root)?;

  let mut flags = MsFlags::empty();
  if readonly {
    flags |= MsFlags::MS_RDONLY;
  }

  mount(
    Some(real_root.as_str()),
    new_root.as_str(),
    fs_type.as_deref(),
    flags,
    mount_data.as_deref(),
  )?;

  for dir in ["/proc", "/sys", "/dev", "/run"] {
    move_mount_if_exists(dir, &new_root)?;
  }

  let old_root = format!("{new_root}/.old_root");
  ensure_dir(&old_root)?;

  pivot_root(new_root.as_str(), old_root.as_str())?;
  chdir("/")?;

  umount2("/.old_root", MntFlags::MNT_DETACH)?;
  let _ = fs::remove_dir_all("/.old_root");
  Ok(())
}

impl Orchestrator for InitramfsOrchestrator {
  fn id(&self) -> &str {
    "initramfs"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::PreBoot],
      phase: BootPhase::End,
    }
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    switch_to_real_root()?;

    let mode = std::env::var("RIND_INITRAMFS_HANDOFF_MODE")
      .unwrap_or("continue".to_string())
      .to_lowercase();

    if mode == "exec" {
      unsafe {
        std::env::set_var("RIND_INITRAMFS_SHORT_CIRCUIT", "1");
      }
    } else {
      unsafe {
        std::env::set_var("RIND_INITRAMFS_SHORT_CIRCUIT", "0");
      }
    }

    Ok(())
  }
}

plugin!(
  name: "initramfs",
  version: 1,
  caps: PluginCapability::ORCHESTRATORS | PluginCapability::INITRD,
  deps: &[],
  create: InitramfsPlugin,
  orchestrators: [InitramfsOrchestrator::default()],
  extensions: [],
  struct InitramfsPlugin;
);

plugin_abi!(1);
