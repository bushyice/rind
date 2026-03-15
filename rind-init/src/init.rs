use std::path::PathBuf;
use std::{fs, time::Duration};

use rind_base::services::{Service, ServiceRuntime};
use rind_core::prelude::*;
use serde_json::json;

const UNITS_META: &str = "units";

struct InitOrchestrator {
  units_dir: PathBuf,
}

impl InitOrchestrator {
  fn new(units_dir: PathBuf) -> Self {
    Self { units_dir }
  }
}

impl Orchestrator for InitOrchestrator {
  fn id(&self) -> &str {
    "init"
  }

  fn depends_on(&self) -> &[String] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect, BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn preload(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    if ctx.metadata.metadata(UNITS_META).is_none() {
      println!("Preloading");
      let source =
        fs::read_to_string(self.units_dir.join("demo.toml")).map_err(CoreError::custom)?;
      let mut metadata = Metadata::new(UNITS_META).of::<Service>("service");
      ctx
        .metadata
        .load_group_from_toml(&mut metadata, "demo", source.as_str())
        .map_err(|e| CoreError::InvalidState(e.to_string()))?;

      println!("{}", metadata.name);

      ctx.metadata.insert_metadata(metadata);
      ctx.metadata.ensure_index_for_type::<Service>("units")?;
      println!("{:?}", ctx.metadata.indexes);
    }

    Ok(())
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("services", "start", json!({ "name": "demo@hello" }))
  }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let units_dir = std::env::temp_dir().join(format!("rind-init-units-{}", std::process::id()));
  let _ = fs::remove_dir_all(units_dir.as_path());
  fs::create_dir_all(units_dir.as_path())?;
  fs::write(
    units_dir.join("demo.toml"),
    r#"
[[service]]
name = "hello"
run = { exec = "/bin/ls", args = ["hello-from-init"] }
"#,
  )?;

  let log = start_logger(LogConfig::default());
  let runtime = start_runtime(log, vec![Box::new(ServiceRuntime::default())]);

  let mut boot = BootEngine::default();
  boot
    .orchestrators
    .push(InitOrchestrator::new(units_dir.clone()));

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  boot
    .run(&mut metadata, &mut instances, &runtime)
    .map_err(|e| format!("{e}"))?;

  // let _ = runtime.send(RuntimeCommand::Stop);
  // let _ = fs::remove_dir_all(units_dir.as_path());

  // std::thread::spawn(|| loop {});

  // loop {
  //   std::thread::park();
  // }
  Ok(())
}
