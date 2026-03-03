use crate::utils::{de_arcstr, s};
use once_cell::sync::Lazy;
use strumbra::SharedString;

pub static CONFIG: Lazy<std::sync::RwLock<InitConfig>> =
  Lazy::new(|| std::sync::RwLock::new(InitConfig::default()));

#[derive(serde::Deserialize)]
pub struct ServicesConfig {
  #[serde(deserialize_with = "de_arcstr")]
  pub path: SharedString,
}

#[derive(serde::Deserialize)]
pub struct ShellConfig {
  #[serde(deserialize_with = "de_arcstr")]
  pub exec: SharedString,
  #[serde(deserialize_with = "de_arcstr")]
  pub tty: SharedString,
}

#[derive(serde::Deserialize)]
pub struct LoggerConfig {
  #[serde(deserialize_with = "de_arcstr")]
  pub socket_path: SharedString,
  #[serde(deserialize_with = "de_arcstr")]
  pub log_path: SharedString,

  pub channel_capacity: usize,
  pub flush_interval: u64, // ms
  pub fsync_interval: u64, // secs
  pub max_segment_size: u64,
  pub batch_size: usize,
}
const MAGIC: u32 = 0x524C4F47; // "RLOG"

#[derive(serde::Deserialize)]
pub struct InitConfig {
  pub services: ServicesConfig,
  pub shell: ShellConfig,
  pub logger: LoggerConfig,
}

impl Default for InitConfig {
  fn default() -> Self {
    Self {
      services: ServicesConfig {
        path: s("/etc/services"),
      },
      shell: ShellConfig {
        exec: s("/bin/sh"),
        tty: s("/dev/tty1"),
      },
      logger: LoggerConfig {
        socket_path: s("/run/rind-logger.sock"),
        log_path: s("/var/log/rind/"),

        channel_capacity: 4096,
        flush_interval: 1,
        max_segment_size: 32 * 1024 * 1024,
        batch_size: 256,
        fsync_interval: 2,
      },
    }
  }
}

impl InitConfig {
  pub fn from_file(file: &str) -> Result<Self, anyhow::Error> {
    let file = std::fs::read_to_string(file)?;
    Ok(toml::from_str(&file)?)
  }
}
