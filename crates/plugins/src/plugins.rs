use std::{
  ops::Deref,
  path::{Path, PathBuf},
};

use libloading::os::unix::{Library, Symbol};
use rind_core::{error::CoreError, prelude::Orchestrator, runtime::RuntimeHandle};

pub use rind_base as base;

bitflags::bitflags! {
  #[repr(C)]
  pub struct PluginCapability: u64 {
    const ALL = 1 << 0;
  }
}

#[repr(C)]
pub struct PluginMetadata {
  pub name: &'static str,
  pub version: u32,
  pub deps: &'static [&'static str],
  pub caps: PluginCapability,
}

pub trait Plugin {
  fn get_metadata(&self) -> PluginMetadata;

  fn provide_orchestrators(&self) -> Vec<Box<dyn Orchestrator>>;
}

pub fn plugins_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_VARIABLES_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/usr/lib/rind/plugins/")
  }
}

pub struct PluginCache {
  pub lib: &'static Library,
  pub meta: PluginMetadata,
  pub plugin: Box<dyn Plugin>,
}

impl Deref for PluginCache {
  type Target = Box<dyn Plugin>;
  fn deref(&self) -> &Self::Target {
    &self.plugin
  }
}

pub fn collect_plugins<P: AsRef<Path>>(
  path: P,
  runtime: &RuntimeHandle,
) -> Result<impl Iterator<Item = PluginCache>, CoreError> {
  // ignore error
  let _ = std::fs::create_dir_all(&path);

  let iter = std::fs::read_dir(path)?
    .filter_map(|entry| entry.ok())
    .map(|entry| entry.path())
    .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("so"))
    .filter_map(|path| unsafe {
      let lib = match Library::new(&path) {
        Ok(l) => l,
        Err(e) => {
          runtime
            .log(
              rind_core::prelude::LogLevel::Error,
              "plugin-loader",
              &format!("Failed to load plugin: {e}"),
              [("name".to_string(), path.to_string_lossy().to_string())].into(),
            )
            .ok();
          return None;
        }
      };

      let version = match lib.get::<u32>(b"PLUGIN_ABI_VERSION") {
        Ok(f) => *f,
        Err(_) => 1,
      };

      let get_plugin: Symbol<unsafe extern "Rust" fn() -> *mut dyn Plugin> =
        match lib.get(b"get_plugin") {
          Ok(s) => s,
          Err(e) => {
            runtime
              .log(
                rind_core::prelude::LogLevel::Error,
                "plugin-loader",
                &format!("Failed to load plugin: {e}"),
                [("name".to_string(), path.to_string_lossy().to_string())].into(),
              )
              .ok();
            return None;
          }
        };

      // bad
      let lib = Box::leak(Box::new(lib));

      let plugin = Box::from_raw(get_plugin());

      let pc = PluginCache {
        lib,
        meta: plugin.get_metadata(),
        plugin,
      };

      runtime
        .log(
          rind_core::prelude::LogLevel::Info,
          "plugin-loader",
          &format!("Loaded plugin"),
          [
            ("name".to_string(), pc.meta.name.to_string()),
            ("path".into(), path.to_string_lossy().to_string()),
            ("abi_version".into(), version.to_string()),
          ]
          .into(),
        )
        .ok();

      Some(pc)
    });

  Ok(iter)
}

pub mod prelude {
  #[allow(ambiguous_glob_reexports)]
  pub use super::*;

  #[macro_export]
  macro_rules! plugin_abi {
    ($abi:expr) => {
      pub const PLUGIN_ABI_VERSION: u32 = $abi;
    };
  }

  #[macro_export]
  macro_rules! plugin {
    (
      name: $name:expr,
      version: $version:expr,
      caps: $caps:expr,
      deps: $deps:expr,

      create: $create:expr,

      orchestrators: [$($body:expr),* $(,)?],

      struct $plugin_name:ident $($body_struct:tt)?
    ) => {
      pub struct $plugin_name $($body_struct)?;

      impl Plugin for $plugin_name {
        fn get_metadata(&self) -> PluginMetadata {
          PluginMetadata {
            name: $name,
            version: $version,
            caps: $caps,
            deps: $deps,
          }
        }

        fn provide_orchestrators(&self) -> Vec<Box<dyn Orchestrator>> {
          vec![$(Box::new($body)),*]
        }
      }

      #[unsafe(no_mangle)]
      pub extern "Rust" fn get_plugin() -> *mut dyn Plugin {
        Box::into_raw(Box::new($create))
      }
    };
  }

  pub use super::base::prelude::*;
}
