use std::{
  collections::{HashMap, HashSet},
  fs::File,
  io::{BufRead, BufReader},
  path::PathBuf,
  sync::Arc,
};

use nix::mount::{MsFlags, mount, umount};
use rind_core::prelude::*;

#[model(meta_name = target, meta_fields(target, source, fstype, flags, data, create, after), derive_metadata(Debug))]
pub struct Mount {
  pub source: Option<Ustr>,
  pub target: Ustr,
  pub fstype: Option<Ustr>,
  pub flags: Option<Vec<String>>,
  pub data: Option<String>,
  pub create: Option<bool>,
  pub after: Option<Vec<Ustr>>,
  pub is_mounted: bool,
}

#[derive(Default)]
pub struct MountRuntime;

impl MountRuntime {
  pub fn umount_target(&self, target: Arc<MountMetadata>) {
    umount(target.target.as_str()).ok();
  }

  pub fn mount_target(&self, target: Arc<MountMetadata>, log: &LogHandle) {
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

    if let Err(e) = mount(
      target.source.as_ref().map(|x| x.as_str()),
      target.target.as_str(),
      target.fstype.as_ref().map(|x| x.as_str()),
      flags,
      target.data.as_deref(),
    ) {
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
  }

  pub fn mount_units(&self, mounts: Vec<(String, Arc<MountMetadata>)>, log: &LogHandle) {
    let mut mounted: HashSet<String> = HashSet::new();
    let mut pending = Vec::new();

    for (idx, (unit_name, mnt)) in mounts.iter().enumerate() {
      let id = mnt.target.clone();
      if let Some(afters) = &mnt.after {
        pending.push((format!("{}@{}", unit_name, mnt.target), afters.clone(), idx));
      } else {
        self.mount_target(mnt.clone(), log);
        mounted.insert(id.to_string());
      }
    }

    loop {
      let mut progress = false;

      pending.retain(|(mount_name, afters, idx)| {
        if afters.iter().all(|a| mounted.contains(a.as_str())) {
          if let Some((_, mnt)) = mounts.get(*idx) {
            self.mount_target(mnt.clone(), log);
            mounted.insert(mount_name.clone());
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
  }
}

fn parse_mount_flags(items: Option<&[String]>) -> MsFlags {
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

impl Runtime for MountRuntime {
  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    match action {
      "mount" => {
        let name = payload.get::<String>("name")?;
        let metadata = ctx
          .registry
          .metadata
          .find::<Mount>("units", &name)
          .ok_or(CoreError::MissingSchema { name })?;
        self.mount_target(metadata, log);
      }
      "umount" => {
        let name = payload.get::<String>("name")?;
        let metadata = ctx
          .registry
          .metadata
          .find::<Mount>("units", &name)
          .ok_or(CoreError::MissingSchema { name })?;
        self.umount_target(metadata);
      }
      "mount_all" => {
        let m = ctx
          .registry
          .metadata
          .metadata("units")
          .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;

        let mut all_mounts: Vec<(String, Arc<MountMetadata>)> = Vec::new();
        for group in m.groups() {
          if let Some(mounts) = ctx
            .registry
            .metadata
            .group_items::<Mount>("units", group.clone())
          {
            for mnt in mounts {
              all_mounts.push((group.to_string(), mnt));
            }
          }
        }
        self.mount_units(all_mounts, log);
      }
      _ => {}
    }
    Ok(None)
  }

  fn id(&self) -> &str {
    "mounts"
  }
}
