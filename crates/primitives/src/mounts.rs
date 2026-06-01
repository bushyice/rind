use std::{
  collections::{HashMap, HashSet},
  ffi::CString,
  fs::File,
  io::{BufRead, BufReader},
  path::PathBuf,
  sync::Arc,
};

use nix::{
  errno::Errno,
  mount::{MsFlags, mount, umount},
};
use rind_core::prelude::*;
use serde::{Deserialize, Serialize};

#[model(meta_name = target, meta_fields(target, source, fstype, flags, data, create, after, rind_broadcast), derive_metadata(Debug))]
pub struct Mount {
  pub source: Option<Ustr>,
  pub target: Ustr,
  pub fstype: Option<Ustr>,
  pub flags: Option<Vec<String>>,
  pub data: Option<String>,
  pub create: Option<bool>,
  pub after: Option<Vec<Ustr>>,
  #[serde(default)]
  pub rind_broadcast: bool,
  pub is_mounted: bool,
}

#[derive(Default)]
pub struct MountRuntime;

impl MountRuntime {
  pub fn umount_target(&self, target: Arc<MountMetadata>) {
    umount(target.target.as_str()).ok();
  }

  pub fn mount_target(
    &self,
    target: Arc<MountMetadata>,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    registry: &mut InstanceRegistry<'_>,
  ) -> CoreResult<Void> {
    if let Some(true) = target.create {
      std::fs::create_dir_all(target.target.as_str()).ok();
    }

    let mut fields = HashMap::new();
    fields.insert("target".to_string(), target.target.to_string());
    fields.insert(
      "source".to_string(),
      target
        .source
        .as_ref()
        .map(|x| x.to_string())
        .unwrap_or_default(),
    );
    log.log(LogLevel::Info, "mount-runtime", "Mounting target", fields);

    let flags = parse_mount_flags(target.flags.as_deref());
    // println!("{target:?}");
    let fstype = target.fstype.as_ref().map(|x| x.as_str());

    if let Err(e) = mount(
      target.source.as_ref().map(|x| x.as_str()),
      target.target.as_str(),
      fstype,
      flags,
      target.data.as_deref(),
    ) && !(e == Errno::EBUSY && fstype == Some("devtmpfs") && &**target.target == "/dev")
    // ignore /dev ebusy
    {
      let mut fields = HashMap::new();
      fields.insert("target".to_string(), target.target.to_string());
      fields.insert("error".to_string(), e.to_string());
      log.log(
        LogLevel::Error,
        "mount-runtime",
        "Failed to mount target",
        fields,
      );
    }

    if target.rind_broadcast
      || (target.fstype.is_some()
        && matches!(&target.fstype, Some(fstype) if &**fstype == "devtmpfs" || &**fstype == "sysfs" || &**fstype == "proc"))
    {
      EXTENSIONS
        .with(|extensions| {
          extensions
            .get()
            .expect("extension manager not initialized")
            .resolve("mount", ExtensionExecutionCtx::new(target.clone()))
        })?
        .dispatch(Some(dispatch), Some(log), Some(registry))?;
    }

    Ok(Void)
  }

  pub fn mount_units(
    &self,
    mounts: Vec<(Ustr, Arc<MountMetadata>)>,
    log: &LogHandle,
    dispatch: &RuntimeDispatcher,
    registry: &mut InstanceRegistry<'_>,
  ) -> CoreResult<Void> {
    let mut mounted: HashSet<Ustr> = HashSet::new();
    let mut pending = Vec::new();

    for (idx, (unit_name, mnt)) in mounts.iter().enumerate() {
      let id = mnt.target.clone();
      if let Some(afters) = &mnt.after {
        pending.push((format!("{}:{}", unit_name, mnt.target), afters.clone(), idx));
      } else {
        self.mount_target(mnt.clone(), log, dispatch, registry)?;
        mounted.insert(id);
      }
    }

    loop {
      let mut progress = false;

      pending.retain(|(mount_name, afters, idx)| {
        if afters.iter().all(|a| mounted.contains(a.as_str())) {
          if let Some((_, mnt)) = mounts.get(*idx) {
            let _ = self.mount_target(mnt.clone(), log, dispatch, registry);
            mounted.insert(mount_name.to_ustr());
            progress = true;
          }
          false
        } else {
          true
        }
      });

      if !progress {
        break;
      }
    }

    if !pending.is_empty() {
      log.log(
        LogLevel::Error,
        "mount-runtime",
        "unresolved dependencies",
        pending
          .iter()
          .map(|x| (x.0.clone(), x.1.join(",")))
          .collect(),
      );
    }

    Ok(Void)
  }

  pub fn unmount_units(
    &self,
    mounts: Vec<(Ustr, Arc<MountMetadata>)>,
    log: &LogHandle,
  ) -> CoreResult<Void> {
    let mut unmounted: HashSet<Ustr> = HashSet::new();
    let mut pending = Vec::new();

    let mut dependents: HashMap<Ustr, Vec<Ustr>> = HashMap::new();

    for (_, mnt) in mounts.iter() {
      let target = mnt.target.clone();

      if let Some(afters) = &mnt.after {
        for after in afters {
          dependents
            .entry(after.clone())
            .or_default()
            .push(target.clone());
        }
      }
    }

    for (idx, (_, mnt)) in mounts.iter().enumerate() {
      let target = mnt.target.clone();

      if let Some(deps) = dependents.get(&target) {
        pending.push((target, deps.clone(), idx));
      } else {
        self.umount_target(mnt.clone());
        unmounted.insert(target);
      }
    }

    loop {
      let mut progress = false;

      pending.retain(|(target, deps, idx)| {
        if deps.iter().all(|d| unmounted.contains(d.as_str())) {
          if let Some((_, mnt)) = mounts.get(*idx) {
            self.umount_target(mnt.clone());
            unmounted.insert(target.clone());
            progress = true;
          }
          false
        } else {
          true
        }
      });

      if !progress {
        break;
      }
    }

    if !pending.is_empty() {
      log.log(
        LogLevel::Error,
        "mount-runtime",
        "unresolved reverse dependencies",
        pending
          .iter()
          .map(|x| (x.0.to_string(), x.1.join(",")))
          .collect(),
      );
    }

    Ok(Void)
  }
}

pub fn parse_mount_flags(items: Option<&[String]>) -> MsFlags {
  let Some(items) = items else {
    return MsFlags::empty();
  };

  let mut flags = MsFlags::empty();
  for item in items {
    let flag = match item.as_str() {
      "MS_RDONLY" => MsFlags::MS_RDONLY,
      "MS_NOSUID" => MsFlags::MS_NOSUID,
      "MS_NODEV" => MsFlags::MS_NODEV,
      "MS_NOEXEC" => MsFlags::MS_NOEXEC,
      "MS_RELATIME" => MsFlags::MS_RELATIME,
      "MS_BIND" => MsFlags::MS_BIND,
      "MS_REC" => MsFlags::MS_REC,
      "MS_PRIVATE" => MsFlags::MS_PRIVATE,
      "MS_SHARED" => MsFlags::MS_SHARED,
      "MS_SLAVE" => MsFlags::MS_SLAVE,
      "MS_STRICTATIME" => MsFlags::MS_STRICTATIME,
      "MS_LAZYTIME" => MsFlags::MS_LAZYTIME,
      _ => MsFlags::empty(),
    };
    flags |= flag;
  }
  flags
}

pub fn is_mounted(target: impl Into<PathBuf>) -> std::io::Result<bool> {
  let file = File::open("/proc/self/mountinfo")?;
  let reader = BufReader::new(file);
  let target = target.into();

  for line in reader.lines() {
    let line = line?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() > 4 && parts[4] == target.to_string_lossy() {
      return Ok(true);
    }
  }
  Ok(false)
}

#[runtime("mounts")]
impl MountRuntime {
  fn mount(&mut self, name: String) {
    let metadata = ctx
      .registry
      .metadata
      .find::<Mount>("*", &name)
      .ok_or(CoreError::MissingSchema(name))?;
    if !is_mounted(metadata.target.as_str())? {
      self.mount_target(metadata, log, dispatch, &mut ctx.registry)?;
    }
  }

  fn umount(&mut self, name: String) {
    let metadata = ctx
      .registry
      .metadata
      .find::<Mount>("*", &name)
      .ok_or(CoreError::MissingSchema(name))?;
    self.umount_target(metadata);
  }

  fn mount_all(&mut self) {
    let mut all_mounts = Vec::new();
    for meta_name in ctx.registry.metadata.metadata_names() {
      let Some(m) = ctx.registry.metadata.metadata(meta_name.clone()) else {
        continue;
      };
      for group in m.groups() {
        if let Some(mounts) = ctx
          .registry
          .metadata
          .group_items::<Mount>(meta_name.clone(), group.clone())
        {
          for mnt in mounts {
            all_mounts.push((group.clone(), mnt));
          }
        }
      }
    }
    self.mount_units(all_mounts, log, dispatch, &mut ctx.registry)?;
  }

  fn mount_all_for(&mut self, scope: Ustr) {
    self.mount_units(
      ctx
        .registry
        .metadata
        .items::<Mount>(scope)
        .unwrap_or_default(),
      log,
      dispatch,
      &mut ctx.registry,
    )?;
  }

  fn unmount_all_for(&mut self, scope: Ustr) {
    self.unmount_units(
      ctx
        .registry
        .metadata
        .items::<Mount>(scope)
        .unwrap_or_default(),
      log,
    )?;
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceMountEntry {
  pub source: Option<String>,
  pub target: String,
  pub fstype: Option<String>,
  #[serde(default)]
  pub flags: Option<Vec<String>>,
  pub data: Option<String>,
  #[serde(default)]
  pub create: Option<bool>,
}

pub fn mount_all_in_namespace(entries: &[NamespaceMountEntry]) -> std::io::Result<()> {
  for entry in entries {
    if entry.create.unwrap_or(false) {
      let _ = std::fs::create_dir_all(&entry.target);
    }
    let flags = parse_mount_flags(entry.flags.as_deref());
    let source = entry.source.as_deref().and_then(|s| CString::new(s).ok());
    let target = CString::new(entry.target.as_str())
      .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let fstype = entry.fstype.as_deref().and_then(|s| CString::new(s).ok());
    let data = entry.data.as_deref().and_then(|s| CString::new(s).ok());
    let rc = unsafe {
      libc::mount(
        source.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
        target.as_ptr(),
        fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
        flags.bits(),
        data
          .as_ref()
          .map_or(std::ptr::null(), |s| s.as_ptr() as *const libc::c_void),
      )
    };
    if rc < 0 {
      let err = std::io::Error::last_os_error();
      if let Some(ref fstype) = entry.fstype {
        if fstype == "devtmpfs" && entry.target == "/dev" {
          continue;
        }
      }
      return Err(err);
    }
  }
  Ok(())
}
