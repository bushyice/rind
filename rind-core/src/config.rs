use once_cell::sync::Lazy;

pub static CONFIG: Lazy<std::sync::RwLock<InitConfig>> =
  Lazy::new(|| std::sync::RwLock::new(InitConfig::default()));

#[derive(serde::Deserialize)]
pub struct ServicesConfig {
  pub path: String,
}

#[derive(serde::Deserialize)]
pub struct ShellConfig {
  pub exec: String,
  pub tty: String,
}

#[derive(serde::Deserialize)]
pub struct InitConfig {
  pub services: ServicesConfig,
  pub shell: ShellConfig,
}

impl Default for InitConfig {
  fn default() -> Self {
    Self {
      services: ServicesConfig {
        path: "/etc/services".into(),
      },
      shell: ShellConfig {
        exec: "/bin/sh".into(),
        tty: "tty1".into(),
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
