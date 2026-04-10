use std::collections::HashMap;

pub use rind_plugins::prelude::*;

struct MyOrchestrator;

impl Orchestrator for MyOrchestrator {
  fn id(&self) -> &str {
    "myorc"
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

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx
      .runtime
      .log(LogLevel::Info, "myplugin", "plugin loaded", HashMap::new())?;
    Ok(())
  }
}

plugin!(
  name: "myplugin",
  version: 0,
  caps: PluginCapability::all(),
  deps: &[],
  create: MyPlugin,
  orchestrators: [MyOrchestrator],
  struct MyPlugin
);

plugin_abi!(1);
