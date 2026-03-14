use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use rind_core::boot::BootEngine;
use rind_core::context::RuntimeContext;
use rind_core::error::CoreError;
use rind_core::logging::{LogHandle, LogLevel};
use rind_core::metadata::{Metadata, Model, NamedItem};
use rind_core::orchestrator::{
  BootCycle, BootPhase, Orchestrator, OrchestratorContext, OrchestratorWhen,
};
use rind_core::registry::InstanceRegistry;
use rind_core::runtime::{Runtime, RuntimeDispatcher, RuntimePayload};
use serde::{Deserialize, Serialize};
use serde_json::json;

const SERVICE_PLAN_KEY: &str = "service@plans";
const SERVICE_CYCLES: &[BootCycle] = &[BootCycle::Collect, BootCycle::Runtime];

pub struct ServiceModel;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ServiceMetadata {
  pub name: String,
  pub run: Option<toml::Value>,
  pub after: Option<Vec<String>>,
}

impl NamedItem for ServiceMetadata {
  fn name(&self) -> &str {
    self.name.as_str()
  }
}

impl Model for ServiceModel {
  type M = ServiceMetadata;
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ExecRule {
  pub exec: String,
  pub args: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStartPlan {
  pub full_name: String,
  pub exec: String,
  pub args: Vec<String>,
}

pub fn services_metadata(metadata_name: impl Into<String>) -> Metadata {
  Metadata::new(metadata_name).of::<ServiceModel>("service")
}

pub fn install_service_example(boot: &mut BootEngine, units_dir: impl Into<PathBuf>) {
  boot
    .orchestrators
    .push(ServiceOrchestrator::new(units_dir.into()));
}

#[derive(Default)]
pub struct BasicServiceRuntime {
  children: HashMap<String, Child>,
}

impl Runtime for BasicServiceRuntime {
  fn id(&self) -> &str {
    "service"
  }

  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    _ctx: &RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<(), CoreError> {
    match action {
      "start" => {
        let name = payload.get::<String>("name")?;
        let exec = payload.get::<String>("exec")?;
        let args = payload
          .get::<Option<Vec<String>>>("args")?
          .unwrap_or(Vec::new());

        if self.children.contains_key(&name) {
          return Ok(());
        }

        let child = Command::new(exec.as_str())
          .args(args.as_slice())
          .stdout(Stdio::null())
          .stderr(Stdio::null())
          .spawn()
          .map_err(|err| {
            CoreError::InvalidState(format!("failed to spawn service `{name}`: {err}"))
          })?;

        self.children.insert(name.clone(), child);

        let mut fields = HashMap::new();
        fields.insert("service".to_string(), name);
        fields.insert("action".to_string(), "start".to_string());
        log.log(LogLevel::Info, "service-runtime", "service started", fields);
      }
      "stop" => {
        let name = payload.get::<String>("name")?;

        if let Some(mut child) = self.children.remove(&name) {
          let _ = child.kill();
          let _ = child.wait();
        }

        let mut fields = HashMap::new();
        fields.insert("service".to_string(), name);
        fields.insert("action".to_string(), "stop".to_string());
        log.log(LogLevel::Info, "service-runtime", "service stopped", fields);
      }
      _ => {}
    }

    Ok(())
  }
}

struct ServiceOrchestrator {
  units_dir: PathBuf,
  depends_on: Vec<String>,
}

impl ServiceOrchestrator {
  fn new(units_dir: PathBuf) -> Self {
    Self {
      units_dir,
      depends_on: Vec::new(),
    }
  }
}

impl Orchestrator for ServiceOrchestrator {
  fn id(&self) -> &str {
    "service"
  }

  fn depends_on(&self) -> &[String] {
    self.depends_on.as_slice()
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: SERVICE_CYCLES,
      phase: BootPhase::Start,
    }
  }

  fn preload(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    if ctx.registry.metadata.metadata("units").is_none() {
      ctx
        .registry
        .metadata
        .insert_metadata(services_metadata("units"));
    }

    let mut plans = Vec::<ServiceStartPlan>::new();
    for (group, source) in read_units(self.units_dir.as_path())? {
      ctx
        .registry
        .metadata
        .load_group_from_toml("units", group.as_str(), source.as_str())
        .map_err(|err| {
          CoreError::InvalidState(format!("failed to load unit group `{group}`: {err}"))
        })?;

      plans.extend(parse_plans_from_source(group.as_str(), source.as_str())?);
    }

    ctx
      .registry
      .instances
      .insert(SERVICE_PLAN_KEY.to_string(), vec![Box::new(plans)]);

    Ok(())
  }

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    for plan in collected_plans(ctx.registry) {
      ctx.dispatch(
        "service",
        "start",
        json!({
          "name": plan.full_name,
          "exec": plan.exec,
          "args": plan.args,
        }),
      )?;
      ctx.dispatch("service", "stop", json!({ "name": plan.full_name }))?;
    }

    Ok(())
  }
}

fn read_units(path: &Path) -> Result<Vec<(String, String)>, CoreError> {
  let entries = fs::read_dir(path).map_err(|err| {
    CoreError::InvalidState(format!(
      "failed to read units dir `{}`: {err}",
      path.display()
    ))
  })?;
  let mut out = Vec::new();

  for entry in entries.flatten() {
    let file_path = entry.path();
    if file_path.extension().and_then(|x| x.to_str()) != Some("toml") {
      continue;
    }

    let Some(stem) = file_path.file_stem().and_then(|x| x.to_str()) else {
      continue;
    };

    let source = fs::read_to_string(file_path.as_path()).map_err(|err| {
      CoreError::InvalidState(format!(
        "failed to read unit file `{}`: {err}",
        file_path.display()
      ))
    })?;
    out.push((stem.to_string(), source));
  }

  Ok(out)
}

fn parse_plans_from_source(group: &str, source: &str) -> Result<Vec<ServiceStartPlan>, CoreError> {
  let table: toml::Value = toml::from_str(source)
    .map_err(|err| CoreError::InvalidState(format!("failed to parse `{group}` source: {err}")))?;
  let Some(raw_service) = table.get("service") else {
    return Ok(Vec::new());
  };

  let services = raw_service
    .clone()
    .try_into::<Vec<ServiceMetadata>>()
    .map_err(|err| CoreError::InvalidState(format!("failed to parse `service` entries: {err}")))?;

  let mut plans = Vec::new();
  for service in services {
    let Some(rule) = first_exec_rule(service.run.as_ref()) else {
      continue;
    };
    plans.push(ServiceStartPlan {
      full_name: format!("{group}@{}", service.name),
      exec: rule.exec,
      args: rule.args.unwrap_or_default(),
    });
  }

  Ok(plans)
}

fn collected_plans(registry: &InstanceRegistry) -> Vec<ServiceStartPlan> {
  registry
    .instances
    .get(SERVICE_PLAN_KEY)
    .and_then(|entries| entries.last())
    .and_then(|boxed| boxed.downcast_ref::<Vec<ServiceStartPlan>>())
    .cloned()
    .unwrap_or_default()
}

fn first_exec_rule(run: Option<&toml::Value>) -> Option<ExecRule> {
  let run = run?.clone();

  if let Ok(one) = run.clone().try_into::<ExecRule>() {
    return Some(one);
  }
  if let Ok(many) = run.try_into::<Vec<ExecRule>>() {
    return many.first().cloned();
  }

  None
}

#[cfg(test)]
mod tests {
  use std::sync::mpsc;
  use std::time::Duration;

  use rind_core::logging::{LogConfig, start_logger};
  use rind_core::runtime::{
    Runtime, RuntimeCommand, RuntimeDispatcher, RuntimePayload, start_runtime,
  };

  use super::*;

  struct CaptureRuntime {
    tx: mpsc::Sender<String>,
  }

  impl Runtime for CaptureRuntime {
    fn id(&self) -> &str {
      "service"
    }

    fn handle(
      &mut self,
      action: &str,
      payload: RuntimePayload,
      _ctx: &RuntimeContext<'_>,
      _dispatch: &RuntimeDispatcher,
      _log: &LogHandle,
    ) -> Result<(), CoreError> {
      let name = payload.get::<String>("name")?;
      let _ = self.tx.send(format!("{action}:{name}"));
      Ok(())
    }
  }

  #[test]
  fn service_orchestrators_dispatch_start_and_stop() {
    let tmp = std::env::temp_dir().join(format!("rind-base-svc-{}", std::process::id()));
    let _ = fs::remove_dir_all(tmp.as_path());
    fs::create_dir_all(tmp.as_path()).expect("temp dir should be created");
    fs::write(
      tmp.join("demo.toml"),
      r#"
[[service]]
name = "sleepy"
run = { exec = "/bin/sleep", args = ["1"] }
"#,
    )
    .expect("unit file should be written");

    let log = start_logger(LogConfig::default());
    let (tx, rx) = mpsc::channel::<String>();
    let runtime = start_runtime(log, vec![Box::new(CaptureRuntime { tx })]);

    let mut boot = BootEngine::default();
    install_service_example(&mut boot, tmp.clone());
    let mut registry = InstanceRegistry::default();
    boot
      .run(&mut registry, &runtime)
      .expect("boot should dispatch service actions");

    assert_eq!(collected_plans(&registry).len(), 1);

    let first = rx
      .recv_timeout(Duration::from_secs(2))
      .expect("start dispatch should be emitted");
    let second = rx
      .recv_timeout(Duration::from_secs(2))
      .expect("stop dispatch should be emitted");

    assert_eq!(first, "start:demo@sleepy".to_string());
    assert_eq!(second, "stop:demo@sleepy".to_string());

    let _ = runtime.send(RuntimeCommand::Stop);
    let _ = fs::remove_dir_all(tmp.as_path());
  }

  #[test]
  fn metadata_registry_loads_service_model() {
    let mut registry = InstanceRegistry::default();
    registry
      .metadata
      .insert_metadata(services_metadata("units"));
    registry
      .metadata
      .load_group_from_toml(
        "units",
        "demo",
        r#"
[[service]]
name = "sleepy"
run = { exec = "/bin/sleep", args = ["1"] }
"#,
      )
      .expect("group should load");

    let services = registry
      .metadata
      .group_items::<ServiceModel>("units", "demo")
      .expect("service group should exist");
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].name, "sleepy".to_string());
  }
}
