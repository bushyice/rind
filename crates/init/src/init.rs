use rind_cfg::prelude::*;
use rind_core::prelude::*;
use rind_plugins::{PluginCapability, collect_plugins, plugins_path};
use std::path::PathBuf;

use crate::pump::setup_event_loop;

mod early;
mod fstab;
mod initramfs;
mod pump;

struct BootOrchestrator;

impl Orchestrator for BootOrchestrator {
  fn id(&self) -> &str {
    "boot"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    ctx.dispatch("mounts", "mount_all", Default::default())?;

    ctx.dispatch("user", "create_sessions", Default::default())?;

    ctx.dispatch("events", "watch_events", Default::default())?;

    ctx.dispatch("ipc", "init_actions", Default::default())?;
    ctx.dispatch("ipc", "start_server", Default::default())?;

    ctx.dispatch("flow", "bootstrap", Default::default())?;
    ctx.dispatch("transport", "bootstrap", Default::default())?;
    ctx.dispatch("sockets", "bootstrap", Default::default())?;
    ctx.dispatch("services", "bootstrap", Default::default())?;

    ctx.dispatch(
      "flow",
      "impulse",
      RuntimePayload::default()
        .insert("name", "rind:boot".to_ustr())
        .insert("payload", serde_json::Value::String("".into())),
    )?;

    ctx.dispatch(
      "flow",
      "set_facet",
      RuntimePayload::default().insert("name", "rind:up!".to_ustr()),
    )?;

    Ok(Void)
  }
}

struct AfterBootOrchestrator;

impl Orchestrator for AfterBootOrchestrator {
  fn id(&self) -> &str {
    "after-boot"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Runtime],
      phase: BootPhase::End,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    ctx.dispatch("sockets", "setup_all", Default::default())?;
    ctx.dispatch("services", "start_all", Default::default())?;
    ctx.dispatch("events", "evaluate_triggers", Default::default())?;

    Ok(Void)
  }
}

struct RuntimeProviderOrchestrator;

impl Orchestrator for RuntimeProviderOrchestrator {
  fn id(&self) -> &str {
    "runtime-provider"
  }

  fn depends_on(&self) -> &[&str] {
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
      Box::new(UserRuntime::default()),
      Box::new(SocketRuntime::default()),
      Box::new(TimerRuntime::default()),
      Box::new(EventsRuntime::default()),
    ]
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    Ok(Void)
  }
}

struct PumpOrchestrator;

impl Orchestrator for PumpOrchestrator {
  fn id(&self) -> &str {
    "pump"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Pump],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    ctx.dispatch("reaper", "reap_once", Default::default())?;
    ctx.dispatch("reaper", "timeout_sweep", Default::default())?;
    ctx.dispatch("events", "drain_events", Default::default())?;
    ctx.dispatch("transport", "drain_incoming", Default::default())?;
    ctx.dispatch("ipc", "drain_requests", Default::default())?;
    Ok(Void)
  }
}

fn main() -> Result<Void, Box<dyn std::error::Error>> {
  std::panic::set_hook(Box::new(|info| {
    let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
      s.to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
      s.clone()
    } else {
      "unknown panic".to_string()
    };
    eprintln!("[rind] FATAL: {msg}");
    if let Some(loc) = info.location() {
      eprintln!("  at {}:{}", loc.file(), loc.line());
    }
  }));

  if initramfs::should_run_initramfs() {
    let continue_boot = initramfs::initramfs_init()?;
    if !continue_boot {
      return Ok(Void);
    }
  }

  early::load_env();

  early::mount_essential_filesystems().unwrap_or_else(|e| {
    eprintln!("[early] warning: {e}");
  });
  early::create_device_nodes().unwrap_or_else(|e| {
    eprintln!("[early] warning: {e}");
  });
  early::set_hostname().unwrap_or_else(|e| {
    eprintln!("[early] warning: {e}");
  });
  early::mount_fstab().unwrap_or_else(|e| {
    eprintln!("[early] warning: {e}");
  });

  let units_dir = if let Ok(path) = std::env::var("RIND_UNITS_DIR") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/etc/units")
  };

  let pump_interval: u64 = std::env::var("RIND_PUMP_INTERVAL")
    .unwrap_or(15.to_string())
    .parse()
    .unwrap_or(15);

  let mut boot = BootEngine::default();
  let mut extensions = ExtensionManager::default();

  let units = UnitsOrchestrator::new(units_dir);

  let mut metadata = MetadataRegistry::default();
  let mut instances = InstanceMap::default();
  let mut resources = Resources::default();

  let log = boot.start_logger();

  if let Ok(plugins) = match collect_plugins(plugins_path(None), &log, None) {
    Ok(plugins) => Ok(plugins),
    Err(e) => {
      eprintln!("[plugins] failed to load plugins: {e}");
      Err(e)
    }
  } {
    for plugin in plugins {
      if plugin.has_cap(PluginCapability::ORCHESTRATORS) {
        boot.orchestrators.extend(plugin.provide_orchestrators());
      }
      if plugin.has_cap(PluginCapability::EXTENSIONS) {
        plugin.register_extensions(&mut extensions);
      }
      if plugin.has_cap(PluginCapability::EXTENSIBLE) {
        if let Some(ext) = plugin.ext {
          unsafe {
            ext(&extensions);
          };
        }
      }
    }
  }
  EXTENSIONS.with(|e| match e.set(extensions) {
    Ok(_) => {}
    Err(_) => log.log(
      LogLevel::Error,
      "boot",
      "failed to allocate extensions",
      Default::default(),
    ),
  });

  boot.orchestrators.insert(0, PumpOrchestrator);
  boot.orchestrators.insert(0, AfterBootOrchestrator);
  boot.orchestrators.insert(0, BootOrchestrator);
  boot.orchestrators.insert(0, units);
  boot.orchestrators.insert(0, RuntimeProviderOrchestrator);

  let event_loop = setup_event_loop(
    &mut boot,
    &mut metadata,
    &mut instances,
    &mut resources,
    &log,
    pump_interval,
  )?;

  if !event_loop.run(&mut boot, &mut metadata, &mut instances, &mut resources) {
    return Ok(Void);
  }

  Ok(Void)
}
