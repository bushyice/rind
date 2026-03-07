use crate::utils::{de_arcstr, s};
use once_cell::sync::Lazy;
use strumbra::SharedString;

pub static CONFIG: Lazy<std::sync::RwLock<InitConfig>> =
  Lazy::new(|| std::sync::RwLock::new(InitConfig::default()));

#[derive(serde::Deserialize)]
pub struct UnitsConfig {
  #[serde(deserialize_with = "de_arcstr")]
  pub path: SharedString,
  #[serde(deserialize_with = "de_arcstr")]
  pub state: SharedString,
  #[serde(deserialize_with = "de_arcstr")]
  pub fallback: SharedString,
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

#[derive(serde::Deserialize)]
pub struct InitConfig {
  pub units: UnitsConfig,
  pub shell: ShellConfig,
  pub logger: LoggerConfig,
}

impl Default for InitConfig {
  fn default() -> Self {
    Self {
      units: UnitsConfig {
        path: s("/etc/units"),
        state: s("/etc/state"),
        fallback: s("/etc/fallback.toml"),
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

#[cfg(test)]
mod tests {
  use super::InitConfig;
  use std::io::Write;

  #[test]
  fn default_config_has_expected_basics() {
    let cfg = InitConfig::default();
    assert_eq!(cfg.shell.exec.as_str(), "/bin/sh");
    assert!(!cfg.logger.log_path.is_empty());
  }

  #[test]
  fn from_file_parses_toml() {
    let mut path = std::env::temp_dir();
    path.push(format!("rind-config-{}.toml", std::process::id()));
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(
      file,
      r#"
[units]
path = "/etc/units"
state = "/tmp/state.bin"
[shell]
exec = "/bin/sh"
tty = "/dev/tty1"
[logger]
socket_path = "/tmp/rind.sock"
log_path = "/tmp/rind-log"
channel_capacity = 16
flush_interval = 1
fsync_interval = 1
max_segment_size = 1024
batch_size = 8
"#
    )
    .unwrap();

    let parsed = InitConfig::from_file(path.to_str().unwrap()).unwrap();
    assert_eq!(parsed.units.path.as_str(), "/etc/units");
    assert_eq!(parsed.logger.batch_size, 8);

    let _ = std::fs::remove_file(path);
  }
}
