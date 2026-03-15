use std::path::PathBuf;

use rind_base::flow::FlowRuntime;
use rind_base::ipc::IpcRuntime;
use rind_base::mount::MountRuntime;
use rind_base::reaper::ReaperRuntime;
use rind_base::services::ServiceRuntime;
use rind_base::transport::TransportRuntime;
use rind_base::units::UnitsOrchestrator;
use rind_core::prelude::*;
use serde_json::json;

struct BootOrchestrator;

impl Orchestrator for BootOrchestrator {
  fn id(&self) -> &str {
    "boot"
  }

  fn depends_on(&self) -> &[String] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("mounts", "mount_all", json!({}))?;

    ctx.dispatch("services", "watch_events", json!({}))?;

    // ctx.dispatch("services", "start_all", json!({}))?;

    ctx.dispatch("ipc", "start_server", json!({}))?;

    ctx.dispatch("flow", "bootstrap", json!({}))?;

    ctx.dispatch("services", "evaluate_triggers", json!({}))?;

    Ok(())
  }
}

struct RuntimeProviderOrchestrator;

impl Orchestrator for RuntimeProviderOrchestrator {
  fn id(&self) -> &str {
    "runtime-provider"
  }

  fn depends_on(&self) -> &[String] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect],
      phase: BootPhase::Start,
    }
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![
      Box::new(ServiceRuntime::default()),
      Box::new(MountRuntime::default()),
      Box::new(FlowRuntime::default()),
      Box::new(TransportRuntime::default()),
      Box::new(ReaperRuntime::default()),
      Box::new(IpcRuntime::default()),
    ]
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    Ok(())
  }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let units_dir = if let Ok(path) = std::env::var("RIND_UNITS_DIR") {
    PathBuf::from(path)
  } else {
    // will be from config later
    PathBuf::from("/etc/units")
  };

  let mut boot = BootEngine::default();

  boot.orchestrators.push(RuntimeProviderOrchestrator);
  boot.orchestrators.push(UnitsOrchestrator::new(units_dir));
  boot.orchestrators.push(BootOrchestrator);

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let runtime = boot.init_runtime();

  boot
    .run(&mut metadata, &mut instances, &runtime)
    .map_err(|e| format!("boot failed: {e}"))?;

  loop {
    // TODO: Add LoopDispatch boot cycle
    // clean up thread + keep alive
    let _ = runtime.dispatch("reaper", "reap_once", json!({}).into(), 1);
    let _ = runtime.dispatch("reaper", "timeout_sweep", json!({}).into(), 1);
    let _ = runtime.dispatch("services", "drain_events", json!({}).into(), 1);
    let _ = runtime.dispatch("transport", "drain_incoming", json!({}).into(), 1);
    let _ = runtime.dispatch("ipc", "drain_requests", json!({}).into(), 1);

    let _ = runtime.flush_context(1, &mut metadata);
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
}
