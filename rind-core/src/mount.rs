use nix::mount::{MsFlags, mount, umount};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::units::UNITS;

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
  let units = UNITS.read().unwrap();
  for unit in units.enabled() {
    if let Some(ref mounts) = unit.mount {
      for mount in mounts {
        println!("Mounting target: {}", mount.target);
        mount_target(mount);
      }
    }
  }
}
