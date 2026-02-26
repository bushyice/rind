use nix::mount::MsFlags;
use serde::de::Error;
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
pub struct Mount {
  pub source: Option<String>,
  pub target: String,
  pub fstype: Option<String>,
  #[serde(default = "default_flags", deserialize_with = "deserialize_flags")]
  pub flags: MsFlags,
  pub data: Option<String>,
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
