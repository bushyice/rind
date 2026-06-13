use std::{
  cell::RefCell,
  collections::HashMap,
  path::{Path, PathBuf},
};

use once_cell::unsync::OnceCell;
use rind_common::types::{ToUstr, Ustr, Void};
use serde::{Deserialize, Serialize};

use crate::{error::CoreResult, logging::LogHandle};

thread_local! {
  pub static HOOKS: OnceCell<RefCell<HashMap<Ustr, Vec<Hook>>>> = OnceCell::new();
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Hook {
  #[serde(default)]
  pub name: String,
  pub command: String,
  #[serde(default)]
  pub args: Vec<String>,
}

pub fn hooks_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_HOOKS_DIR") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/usr/lib/rind/hooks")
  }
}

fn trigger_hook(hook: Hook, options: Option<&HashMap<&str, &str>>) -> CoreResult<Void> {
  let mut child = std::process::Command::new(hook.command);
  let mut child = child.args(hook.args);

  if let Some(options) = options {
    child = child.envs(options);
  }

  let _ = child.status()?;
  Ok(Void)
}

pub fn trigger_hooks_raw(group: impl Into<Ustr>) {
  let group = group.into();

  let _ = HOOKS.with(|h| {
    for hook in h.get()?.borrow_mut().remove(&group)? {
      trigger_hook(hook.clone(), None).ok()?;
    }

    None::<Void>
  });
}

pub fn trigger_hooks(group: impl Into<Ustr>, log: &LogHandle, options: Option<&[&str]>) {
  let group = group.into();
  let options = options.map(|o| {
    o.iter()
      .filter_map(|o| o.split_once("="))
      .collect::<HashMap<_, _>>()
  });

  let _ = HOOKS.with(|h| {
    for hook in h.get()?.borrow().get(&group)? {
      log.log(
        crate::logging::LogLevel::Info,
        "hooks",
        "running hook",
        [
          ("hook".to_string(), hook.name.clone()),
          ("group".into(), group.to_string()),
        ]
        .into(),
      );
      trigger_hook(hook.clone(), options.as_ref()).ok()?;
    }

    None::<Void>
  });
}

pub fn trigger_hooks_mut(group: impl Into<Ustr>, log: &LogHandle, options: Option<&[&str]>) {
  let group = group.into();
  let options = options.map(|o| {
    o.iter()
      .filter_map(|o| o.split_once("="))
      .collect::<HashMap<_, _>>()
  });

  let _ = HOOKS.with(|h| {
    for hook in h.get()?.borrow_mut().remove(&group)? {
      log.log(
        crate::logging::LogLevel::Info,
        "hooks",
        "running hook",
        [
          ("hook".to_string(), hook.name.clone()),
          ("group".into(), group.to_string()),
        ]
        .into(),
      );
      trigger_hook(hook, options.as_ref()).ok()?;
    }

    None::<Void>
  });
}

pub fn initiate_hooks<P: AsRef<Path>>(path: P) -> CoreResult<Void> {
  HOOKS.with(|h| {
    let mut all: HashMap<Ustr, Vec<Hook>> = HashMap::new();

    if path.as_ref().exists() {
      for (group, path) in std::fs::read_dir(path)?
        .filter_map(|p| p.ok())
        .filter(|p| p.file_type().map_or(false, |d| d.is_dir()))
        .filter_map(|p| {
          let path = p.path();
          let name = path.file_stem()?.to_string_lossy().to_string();
          Some(
            std::fs::read_dir(&path)
              .ok()?
              .filter_map(|x| x.ok())
              .filter(|p| p.file_type().map_or(false, |d| !d.is_dir()))
              .map(move |x| (name.clone(), x.path())),
          )
        })
        .flatten()
      {
        let Ok(command) = std::fs::read_to_string(&path) else {
          continue;
        };

        let mut hook: Hook = toml::from_str(&command)?;

        hook.name = path
          .file_stem()
          .unwrap_or_default()
          .to_string_lossy()
          .to_string();

        all.entry(group.to_ustr()).or_default().push(hook);
      }

      for hooks in all.values_mut() {
        hooks.sort_by(|a, b| a.name.cmp(&b.name));
      }
    }

    if let Some(h) = h.get() {
      *h.borrow_mut() = all;
    } else {
      let _ = h.set(RefCell::new(all));
    }

    Ok(Void)
  })
}
