use std::collections::HashMap;

pub use rind_plugins::prelude::*;

#[model(meta_name = name, meta_fields(name, data), derive_metadata(Debug))]
pub struct MyModel {
  pub name: String,
  pub data: String,
}

struct MyOrchestrator;

impl Orchestrator for MyOrchestrator {
  fn id(&self) -> &str {
    "myorc"
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

  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
    ctx
      .runtime
      .log(LogLevel::Info, "myplugin", "plugin loaded", HashMap::new())?;

    match ctx.dispatch(
      "myruntime",
      "something",
      RuntimePayload::default().insert("thing", "init".to_string()),
    ) {
      Err(e) => ctx.runtime.log(
        LogLevel::Error,
        "myplugin",
        &format!("failed to dispatch {e}"),
        HashMap::new(),
      )?,
      _ => {}
    }
    Ok(())
  }

  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    vec![Box::new(MyRuntime)]
  }
}

pub struct MyRuntime;

impl Runtime for MyRuntime {
  fn id(&self) -> &str {
    "myruntime"
  }

  fn handle(
    &mut self,
    action: &str,
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    let sm = &ctx
      .registry
      .singleton::<StateMachine>(StateMachine::KEY)
      .ok_or_else(|| CoreError::InvalidState("state machine store not found".into()))?
      .states;

    // println!(
    //   "{:?}",
    //   ctx
    //     .registry
    //     .metadata
    //     .lookup::<MyModel>("units", "example@example")
    // );

    log.log(
      LogLevel::Trace,
      "myplugin",
      "logging",
      [
        ("action".to_string(), action.to_string()),
        (
          "payload".to_string(),
          payload.get::<String>("thing").unwrap_or_default(),
        ),
        ("states".into(), sm.len().to_string()),
      ]
      .into(),
    );

    let _ = dispatch.dispatch(
      "flow",
      "set_state",
      FlowRuntimePayload::new("myplugin@state")
        .payload(serde_json::json!({
          "id": 0
        }))
        .into(),
    );

    Ok(None)
  }
}

fn myextension(action: UnitExtensionAction) -> UnitExtensionAction {
  match action {
    UnitExtensionAction::Metadata(units) => units.of::<MyModel>("themodel").into(),
    UnitExtensionAction::CreateIndex => ().into(),
    UnitExtensionAction::LoadedUnits(m) => m.into(),
    UnitExtensionAction::BuiltIn(mut m) => {
      m.from_toml(
        r#"
        [[state]]
        name = "state"
        payload = "json"
      "#,
        "myplugin",
      )
      .ok();
      m.into()
    }
  }
}

plugin!(
  name: "myplugin",
  version: 0,
  caps: PluginCapability::all(),
  deps: &[],
  create: MyPlugin,
  orchestrators: [MyOrchestrator],
  extension: myextension,
  struct MyPlugin;
);

plugin_abi!(1);
