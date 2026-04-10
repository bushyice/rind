/*
 * TODO: Userspace Update
 * - spaces (user/system), give the uid/gid for services.
 * - active/inactive services in start_all
 * - start_all based on the space
 * - Fetch isolated user services from units/username
 */

use std::path::PathBuf;
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use rind_base::flow::FlowRuntime;
use rind_base::ipc::IpcRuntime;
use rind_base::mount::MountRuntime;
use rind_base::networking::NetworkingRuntime;
use rind_base::reaper::ReaperRuntime;
use rind_base::services::ServiceRuntime;
use rind_base::transport::TransportRuntime;
use rind_base::units::UnitsOrchestrator;
use rind_base::user::UserRuntime;
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

    ctx.dispatch("user", "create_sessions", json!({}))?;

    ctx.dispatch("services", "watch_events", json!({}))?;

    // ctx.dispatch("services", "start_all", json!({}))?;

    ctx.dispatch("ipc", "init_actions", json!({}))?;
    ctx.dispatch("ipc", "start_server", json!({}))?;

    ctx.dispatch("firewall", "apply", json!({}))?;

    ctx.dispatch("flow", "bootstrap", json!({}))?;
    ctx.dispatch("networking", "bootstrap", json!({}))?;
    ctx.dispatch("networking", "scan", json!({}))?;
    ctx.dispatch("networking", "configure", json!({}))?;

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
      Box::new(NetworkingRuntime::default()),
      Box::new(ReaperRuntime::default()),
      Box::new(IpcRuntime::default()),
      Box::new(UserRuntime::default()),
    ]
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    Ok(())
  }
}

struct PumpOrchestrator;

impl Orchestrator for PumpOrchestrator {
  fn id(&self) -> &str {
    "pump"
  }

  fn depends_on(&self) -> &[String] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Pump],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx.dispatch("reaper", "reap_once", json!({}))?;
    ctx.dispatch("reaper", "timeout_sweep", json!({}))?;
    ctx.dispatch("networking", "scan", json!({}))?;
    ctx.dispatch("networking", "reconcile", json!({}))?;
    ctx.dispatch("services", "drain_events", json!({}))?;
    ctx.dispatch("transport", "drain_incoming", json!({}))?;
    ctx.dispatch("ipc", "drain_requests", json!({}))?;
    Ok(())
  }
}

fn try_stop_services(
  boot: &BootEngine,
  metadata: &MetadataRegistry,
  runtime: &RuntimeHandle,
  force: bool,
) {
  let Some(context_id) = boot.primary_context_id() else {
    return;
  };

  let _ = runtime.dispatch(
    "services",
    "stop_all",
    json!({ "force": force }).into(),
    context_id,
  );
  let _ = runtime.flush_context(context_id, metadata);
}

fn collect_other_pids() -> Vec<i32> {
  let self_pid = std::process::id() as i32;
  let mut pids = Vec::new();

  let Ok(entries) = std::fs::read_dir("/proc") else {
    return pids;
  };

  for entry in entries.flatten() {
    let name = entry.file_name();
    let name = name.to_string_lossy();
    let Ok(pid) = name.parse::<i32>() else {
      continue;
    };
    if pid <= 1 || pid == self_pid {
      continue;
    }
    pids.push(pid);
  }

  pids
}

fn terminate_all_processes() {
  let pids = collect_other_pids();
  for pid in &pids {
    let _ = kill(Pid::from_raw(*pid), Signal::SIGTERM);
  }

  std::thread::sleep(Duration::from_millis(500));

  let pids = collect_other_pids();
  for pid in &pids {
    let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
  }
}

fn process_lifecycle_action(
  action: LifecycleAction,
  boot: &mut BootEngine,
  metadata: &mut MetadataRegistry,
  instances: &mut InstanceMap,
  runtime: &RuntimeHandle,
) -> bool {
  match action {
    LifecycleAction::ReloadUnits => {
      let _ = boot.reload_units_collection(metadata, instances, runtime);
      true
    }
    LifecycleAction::SoftReboot => {
      try_stop_services(boot, metadata, runtime, false);
      terminate_all_processes();
      metadata.remove_metadata("units");
      let _ = boot.run(metadata, instances, runtime);
      true
    }
    LifecycleAction::Reboot => {
      try_stop_services(boot, metadata, runtime, false);
      terminate_all_processes();
      let _ = runtime.send(RuntimeCommand::Stop);
      unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
      }
      false
    }
    LifecycleAction::Shutdown => {
      try_stop_services(boot, metadata, runtime, true);
      terminate_all_processes();
      let _ = runtime.send(RuntimeCommand::Stop);
      unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF);
      }
      false
    }
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
  let units = UnitsOrchestrator::new(units_dir);
  let lifecycle = units.lifecycle_queue();
  boot.orchestrators.push(units);
  boot.orchestrators.push(BootOrchestrator);
  boot.orchestrators.push(PumpOrchestrator);

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let runtime = boot.init_runtime();

  boot
    .run(&mut metadata, &mut instances, &runtime)
    .map_err(|e| format!("boot failed: {e}"))?;

  loop {
    while let Some(action) = lifecycle.next() {
      if !process_lifecycle_action(action, &mut boot, &mut metadata, &mut instances, &runtime) {
        return Ok(());
      }
    }
    let _ = boot.pump_once(&mut metadata, &mut instances, &runtime);
    std::thread::sleep(std::time::Duration::from_millis(50));
  }
}
