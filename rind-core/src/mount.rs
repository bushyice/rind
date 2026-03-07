use std::collections::HashSet;

use nix::mount::{MsFlags, mount, umount};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::store::STORE;
use rind_common::error::rw_read;
use rind_common::{logerr, loginfo};

#[derive(Deserialize, Serialize)]
pub struct Mount {
  pub source: Option<String>,
  pub target: String,
  pub fstype: Option<String>,
  #[serde(
    default = "default_flags",
    serialize_with = "serialize_flags",
    deserialize_with = "deserialize_flags"
  )]
  pub flags: MsFlags,
  pub data: Option<String>,
  pub create: Option<bool>,
  pub after: Option<Vec<String>>,
}

fn default_flags() -> MsFlags {
  MsFlags::empty()
}

fn deserialize_flags<'de, D>(d: D) -> Result<MsFlags, D::Error>
where
  D: Deserializer<'de>,
{
  let items = Vec::<String>::deserialize(d)?;

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
      _ => return Err(D::Error::custom(format!("unknown mount flag: {item}"))),
    };

    flags |= flag;
  }

  Ok(flags)
}

fn serialize_flags<S>(_flags: &MsFlags, serializer: S) -> Result<S::Ok, S::Error>
where
  S: Serializer,
{
  serializer.collect_seq(Vec::<String>::new())
}

pub fn umount_target(target: &Mount) {
  umount(target.target.as_str()).ok();
}

pub fn mount_target(target: &Mount) {
  if let Some(true) = target.create {
    std::fs::create_dir_all(target.target.clone()).ok();
  }
  loginfo!("Mounting target: {}", target.target);

  mount(
    target.source.as_deref(),
    target.target.as_str(),
    target.fstype.as_deref(),
    target.flags,
    target.data.as_deref(),
  )
  .ok();
}

pub fn mount_units() {
  let store = rw_read(&STORE, "store read in mount_units");

  let mut mounted: HashSet<String> = HashSet::new();
  let mut pending = Vec::new();

  for (unit_name, mount) in store.enabled::<Mount>() {
    let id = mount.target.clone();
    if let Some(afters) = &mount.after {
      pending.push((
        format!("{}@{}", unit_name.to_string(), mount.target.clone()),
        afters.clone(),
      ));
    } else {
      mount_target(mount);
      mounted.insert(id);
    }
  }

  loop {
    let mut progress = false;

    pending.retain(|(mount_name, afters)| {
      if afters.iter().all(|a| mounted.contains(a)) {
        if let Some(mnt) = store.lookup::<Mount>(mount_name) {
          mount_target(mnt);
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
    logerr!(
      "Unresolved dependencies: {:?}",
      pending
        .iter()
        .map(|x| format!("{} for {:?}", x.0, x.1))
        .collect::<Vec<String>>()
    );
  }
}

impl Mount {
  pub fn is_mounted(&self) -> bool {
    crate::utils::is_mounted(&self.target).unwrap_or(false)
  }
}
