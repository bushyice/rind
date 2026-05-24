use std::{
  collections::HashMap,
  path::Path,
  sync::{LazyLock, Mutex},
};

use rind_core::{
  error::{CoreError, CoreResult},
  prelude::{Metadata, OrchestratorContext},
  types::{ToUstr, Ustr, Void},
};

pub type Loader = dyn Fn(&mut Metadata, &str, Ustr, &Path, &mut OrchestratorContext) -> CoreResult<Void>
  + Send
  + Sync;

static LOADERS: LazyLock<Mutex<HashMap<Ustr, Box<Loader>>>> = LazyLock::new(|| {
  let mut defaults: HashMap<Ustr, Box<Loader>> = HashMap::new();

  defaults.insert("toml".to_ustr(), Box::new(toml_loader));
  Mutex::new(defaults)
});

fn toml_loader(
  metadata: &mut Metadata,
  content: &str,
  group: Ustr,
  _path: &Path,
  _ctx: &mut OrchestratorContext<'_>,
) -> CoreResult<Void> {
  metadata.from_toml(&content, group)?;

  Ok(Void)
}

fn load_in_dir(
  ctx: &mut OrchestratorContext<'_>,
  units_dir: &Path,
  metadata: &mut Metadata,
  trigger: &Option<impl Fn(&str, &Ustr, &mut Metadata) -> CoreResult<Void>>,
) -> CoreResult<Void> {
  let dir = std::fs::read_dir(&units_dir).map_err(|e| {
    CoreError::Custom(format!(
      "failed to read units dir {}: {e}",
      units_dir.display()
    ))
  })?;

  for entry in dir {
    let entry = entry.map_err(|e| CoreError::Custom(format!("dir entry error: {e}")))?;
    let path = entry.path();

    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
      load_in_dir(ctx, &path, metadata, trigger)?;
    } else {
      let extension = path.extension().and_then(|x| x.to_str()).unwrap_or("toml");

      let group = Ustr::from(
        path
          .file_stem()
          .and_then(|s| s.to_str())
          .unwrap_or("unknown"),
      );

      let content = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Custom(format!("failed to read unit file {}: {e}", path.display()))
      })?;

      let loaders = LOADERS.lock().unwrap();

      let Some(loader) = loaders.get(&extension.to_ustr()) else {
        continue;
      };

      loader(metadata, &content, group.clone(), units_dir, ctx).map_err(|e| {
        CoreError::Custom(format!("failed to parse unit file {}: {e}", path.display()))
      })?;

      if let Some(trigger) = trigger {
        trigger(&content, &group, metadata)?;
      }

      drop(loaders);
    }
  }

  Ok(Void)
}

pub fn load_units_from(
  ctx: &mut OrchestratorContext<'_>,
  metadata: &mut Metadata,
  units_dir: &Path,
  trigger: Option<impl Fn(&str, &Ustr, &mut Metadata) -> CoreResult<Void>>,
) -> CoreResult<Void> {
  load_in_dir(ctx, units_dir, metadata, &trigger)?;
  Ok(Void)
}

pub fn register_loader(r#type: impl Into<Ustr>, loader: Box<Loader>) {
  let mut loaders = LOADERS.lock().unwrap();
  loaders.insert(r#type.into(), loader);
}

pub struct RegisterLoader;

impl RegisterLoader {
  pub fn register(&self, r#type: impl Into<Ustr>, loader: Box<Loader>) {
    register_loader(r#type, loader);
  }
}
