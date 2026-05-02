// To handle tty, maybe either set the login_required state for each tty only on-access and set a timer to
// remove that state whenever it's not accessed for a while
//
// or maybe, make login_required just a signal instead of a state, and have a timer stop the service
// when it's not being accessed anymore?

use std::{env, fs, sync::Arc};

use rind_plugins::prelude::*;

#[derive(Default)]
struct TTYOrchestrator;

impl Orchestrator for TTYOrchestrator {
  fn id(&self) -> &str {
    "ttys"
  }

  fn depends_on(&self) -> &[&str] {
    &[]
  }

  fn when(&self) -> OrchestratorWhen<'static> {
    OrchestratorWhen {
      cycle: &[BootCycle::Collect, BootCycle::Runtime],
      phase: BootPhase::Start,
    }
  }

  fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    Ok(())
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![Box::new(TTYRuntime::default())]
  }
}

#[derive(Default)]
pub struct TTYRuntime;

impl Runtime for TTYRuntime {
  fn id(&self) -> &str {
    "ttys"
  }

  fn handle(
    &mut self,
    _action: &str,
    _payload: RuntimePayload,
    _ctx: &mut RuntimeContext<'_>,
    _dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    Ok(None)
  }
}

fn trigger_ttyload(
  name: &str,
  ctx: ExtensionExecutionCtx<Arc<MountMetadata>>,
) -> CoreResult<ExtensionExecutionCtx<Arc<MountMetadata>>> {
  match name {
    "mount" if ctx.target.target.as_str() == "/sys" => {
      Ok(ctx.with_fn(|_, _, registry| {
        let Some(registry) = registry else {
          return Err(CoreError::Unknown);
        };
        let mut ttys: Vec<Ustr> = Vec::new();
        let mut tty_count = 0;

        let limit = env::var("RIND_TTY_LIMIT")
          .ok()
          .and_then(|v| v.parse::<usize>().ok())
          .unwrap_or(7);

        if let Ok(dir) = fs::read_dir("/sys/class/tty") {
          let mut entries: Vec<_> = dir.collect::<Result<Vec<_>, _>>()?;
          entries.sort_by_key(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();

            name
              .strip_prefix("tty")
              .and_then(|n| n.parse::<u32>().ok())
              .unwrap_or(u32::MAX)
          });

          for item in entries {
            let name = item.file_name();
            let name = name.to_string_lossy();

            // TODO: proper tty fetch
            if name.starts_with("tty") && name != "tty" && name != "tty0" && tty_count < limit {
              ttys.push(format!("/dev/{}", name).into());
              tty_count += 1;
            }
          }
        }

        if let Some(vh) = registry.singleton_mut::<VariableHeap>(VariableHeap::KEY) {
          vh.set(
            "ttys",
            toml::Value::Array(
              ttys
                .iter()
                .map(|x| toml::Value::String(x.to_string()))
                .collect(),
            ),
          );
        }

        Ok(Box::new(()))
      }))
    }
    _ => Ok(ctx),
  }
}

plugin!(
  name: "myplugin",
  version: 0,
  caps: PluginCapability::all(),
  deps: &[],
  create: MyPlugin,
  orchestrators: [TTYOrchestrator::default()],
  extensions: [resolve(trigger_ttyload)],
  struct MyPlugin;
);

plugin_abi!(1);
