// use serde::{Deserialize, Serialize};
// use std::collections::HashMap;
// use std::fs;
// use std::path::{Path, PathBuf};

// use rind_core::prelude::*;

// use crate::services::Service;

// const FLOW_RUNTIME_ID: &str = "flow";
// const FLOW_ORCHESTRATOR_ID: &str = "flow";
// const UNITS_ORCHESTRATOR_ID: &str = "units";

// #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
// #[serde(untagged)]
// pub enum FlowItem {
//   Simple(String),
//   Detailed {
//     state: Option<String>,
//     signal: Option<String>,
//     target: Option<FlowMatchOperation>,
//     branch: Option<FlowMatchOperation>,
//   },
// }

// #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
// #[serde(untagged)]
// pub enum FlowMatchOperation {
//   Eq(String),
//   Options {
//     binary: Option<bool>,
//     contains: Option<String>,
//     r#as: Option<serde_json::Value>,
//   },
// }

// #[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
// #[serde(rename_all = "snake_case")]
// pub enum FlowType {
//   #[default]
//   Signal,
//   State,
// }

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub struct FlowJson(pub String);

// impl From<String> for FlowJson {
//   fn from(value: String) -> Self {
//     Self(value)
//   }
// }

// impl FlowJson {
//   pub fn into_json(&self) -> serde_json::Value {
//     serde_json::from_str(&self.0).unwrap_or(serde_json::Value::Null)
//   }
// }

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub enum FlowPayload {
//   Json(FlowJson),
//   String(String),
//   Bytes(Vec<u8>),
//   None(bool),
// }

// impl FlowPayload {
//   pub fn to_string_payload(&self) -> String {
//     match self {
//       FlowPayload::Json(v) => v.0.clone(),
//       FlowPayload::String(v) => v.clone(),
//       FlowPayload::Bytes(v) => String::from_utf8(v.clone()).unwrap_or_default(),
//       FlowPayload::None(_) => String::new(),
//     }
//   }

//   pub fn to_json(&self) -> serde_json::Value {
//     match self {
//       FlowPayload::Json(v) => v.into_json(),
//       FlowPayload::String(v) => serde_json::Value::String(v.clone()),
//       FlowPayload::Bytes(v) => serde_json::json!(v),
//       FlowPayload::None(_) => serde_json::Value::Null,
//     }
//   }

//   pub fn from_json(v: Option<serde_json::Value>) -> Self {
//     match v {
//       Some(serde_json::Value::Object(v)) => {
//         FlowPayload::Json(FlowJson(serde_json::Value::Object(v).to_string()))
//       }
//       Some(serde_json::Value::Array(v)) => {
//         FlowPayload::Json(FlowJson(serde_json::Value::Array(v).to_string()))
//       }
//       Some(serde_json::Value::String(v)) => FlowPayload::String(v),
//       Some(serde_json::Value::Null) | None => FlowPayload::None(false),
//       Some(v) => FlowPayload::String(v.to_string()),
//     }
//   }
// }

// #[derive(Debug, Serialize, Deserialize, Clone)]
// pub struct FlowInstance {
//   pub name: String,
//   pub payload: FlowPayload,
//   pub r#type: FlowType,
// }

// #[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
// #[serde(rename_all = "snake_case")]
// pub enum FlowPayloadType {
//   #[default]
//   Json,
//   String,
//   Bytes,
//   None,
// }

// #[derive(Debug, Serialize, Deserialize, Default, Clone)]
// pub struct FlowDefinitionBase {
//   pub name: String,
//   pub payload: FlowPayloadType,
//   #[serde(rename = "activate-on-none")]
//   pub activate_on_none: Option<Vec<String>>,
//   pub after: Option<Vec<FlowItem>>,
//   pub branch: Option<Vec<String>>,
// }

// #[model(meta_name = name, meta_fields(name, payload, activate_on_none, after, branch), derive_metadata(Debug, Clone))]
// pub struct State {
//   pub name: String,
//   pub payload: FlowPayloadType,
//   #[serde(rename = "activate-on-none")]
//   pub activate_on_none: Option<Vec<String>>,
//   pub after: Option<Vec<FlowItem>>,
//   pub branch: Option<Vec<String>>,
// }

// #[model(meta_name = name, meta_fields(name, payload), derive_metadata(Debug, Clone))]
// pub struct Signal {
//   pub name: String,
//   pub payload: FlowPayloadType,
// }

// #[derive(Default)]
// pub struct FlowRuntime {
//   states: HashMap<String, Vec<FlowInstance>>,
// }

// impl FlowRuntime {
//   fn payload_type_ok(expected: FlowPayloadType, payload: &FlowPayload) -> bool {
//     match payload {
//       FlowPayload::Json(_) => expected == FlowPayloadType::Json,
//       FlowPayload::String(_) => expected == FlowPayloadType::String,
//       FlowPayload::Bytes(_) => expected == FlowPayloadType::Bytes,
//       FlowPayload::None(_) => expected == FlowPayloadType::None,
//     }
//   }

//   fn check_condition(cond: &FlowItem, trigger: &FlowInstance) -> bool {
//     match cond {
//       FlowItem::Simple(name) => *name == trigger.name,
//       FlowItem::Detailed {
//         state,
//         signal,
//         target,
//         branch,
//       } => {
//         if let Some(state) = state {
//           if trigger.r#type != FlowType::State || *state != trigger.name {
//             return false;
//           }
//           if let Some(branch) = branch {
//             return Self::match_op(branch, &trigger.payload);
//           }
//           true
//         } else if let Some(signal) = signal {
//           if trigger.r#type != FlowType::Signal || *signal != trigger.name {
//             return false;
//           }
//           if let Some(target) = target {
//             return Self::match_op(target, &trigger.payload);
//           }
//           true
//         } else {
//           false
//         }
//       }
//     }
//   }

//   fn match_op(op: &FlowMatchOperation, payload: &FlowPayload) -> bool {
//     match op {
//       FlowMatchOperation::Eq(v) => payload.to_string_payload() == *v,
//       FlowMatchOperation::Options {
//         binary,
//         contains,
//         r#as,
//       } => {
//         if let Some(true) = binary {
//           matches!(payload, FlowPayload::Bytes(_))
//         } else if let Some(contains) = contains {
//           payload.to_string_payload().contains(contains)
//         } else if let Some(filter) = r#as {
//           subset_match(filter, &payload.to_json())
//         } else {
//           false
//         }
//       }
//     }
//   }

//   fn dispatch_service_transitions(
//     &self,
//     trigger: &FlowInstance,
//     mode_apply: bool,
//     ctx: &mut RuntimeContext<'_>,
//     dispatch: &RuntimeDispatcher,
//   ) -> Result<(), CoreError> {
//     let metadata = ctx
//       .registry
//       .metadata
//       .metadata("units")
//       .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;

//     for group in metadata.groups() {
//       let services = ctx
//         .registry
//         .metadata
//         .group_items::<Service>("units", group)
//         .cloned()
//         .unwrap_or_default();

//       for svc in services {
//         let name = format!("{group}@{}", svc.name);

//         if let Some(start_on) = svc.start_on {
//           if start_on
//             .iter()
//             .any(|cond| Self::check_condition(cond, trigger))
//           {
//             dispatch.dispatch(
//               "services",
//               if mode_apply { "start" } else { "stop" },
//               serde_json::json!({ "name": name.clone() }).into(),
//             )?;
//           }
//         }

//         if let Some(stop_on) = svc.stop_on {
//           if stop_on
//             .iter()
//             .any(|cond| Self::check_condition(cond, trigger))
//           {
//             dispatch.dispatch(
//               "services",
//               if mode_apply { "stop" } else { "start" },
//               serde_json::json!({ "name": name }).into(),
//             )?;
//           }
//         }
//       }
//     }

//     Ok(())
//   }

//   fn activate_on_none_boot(
//     &mut self,
//     ctx: &mut RuntimeContext<'_>,
//     dispatch: &RuntimeDispatcher,
//   ) -> Result<(), CoreError> {
//     let metadata = ctx
//       .registry
//       .metadata
//       .metadata("units")
//       .ok_or_else(|| CoreError::MetadataNotFound("units".to_string()))?;

//     let mut to_set = Vec::<String>::new();
//     for group in metadata.groups() {
//       let states = ctx
//         .registry
//         .metadata
//         .group_items::<State>("units", group)
//         .cloned()
//         .unwrap_or_default();

//       for state in states {
//         let Some(deps) = state.activate_on_none else {
//           continue;
//         };

//         let should_activate = deps
//           .iter()
//           .all(|dep| self.states.get(dep).map(|v| v.is_empty()).unwrap_or(true));

//         if should_activate {
//           to_set.push(format!("{group}@{}", state.name));
//         }
//       }
//     }

//     for name in to_set {
//       let payload = FlowPayload::None(false);
//       self.states.insert(
//         name.clone(),
//         vec![FlowInstance {
//           name: name.clone(),
//           payload: payload.clone(),
//           r#type: FlowType::State,
//         }],
//       );

//       self.dispatch_service_transitions(
//         &FlowInstance {
//           name,
//           payload,
//           r#type: FlowType::State,
//         },
//         true,
//         ctx,
//         dispatch,
//       )?;
//     }

//     Ok(())
//   }
// }

// impl Runtime for FlowRuntime {
//   fn id(&self) -> &str {
//     FLOW_RUNTIME_ID
//   }

//   fn handle(
//     &mut self,
//     action: &str,
//     payload: RuntimePayload,
//     ctx: &mut RuntimeContext<'_>,
//     dispatch: &RuntimeDispatcher,
//     _log: &LogHandle,
//   ) -> Result<(), CoreError> {
//     match action {
//       "bootstrap" => {
//         self.activate_on_none_boot(ctx, dispatch)?;
//       }
//       "set_state" => {
//         let name = payload.get::<String>("name")?;
//         let flow_payload = FlowPayload::from_json(payload.0.get("payload").cloned());

//         let def = ctx
//           .registry
//           .metadata
//           .lookup::<State>("units", &name)
//           .ok_or_else(|| CoreError::InvalidState(format!("state not found: {name}")))?;

//         if !Self::payload_type_ok(def.payload, &flow_payload) {
//           return Err(CoreError::InvalidState(format!(
//             "state payload type mismatch: {name}"
//           )));
//         }

//         let instance = FlowInstance {
//           name: name.clone(),
//           payload: flow_payload,
//           r#type: FlowType::State,
//         };

//         self.states.insert(name, vec![instance.clone()]);
//         self.dispatch_service_transitions(&instance, true, ctx, dispatch)?;
//       }
//       "remove_state" => {
//         let name = payload.get::<String>("name")?;
//         if let Some(mut removed) = self.states.remove(&name) {
//           for mut instance in removed.drain(..) {
//             instance.r#type = FlowType::State;
//             self.dispatch_service_transitions(&instance, false, ctx, dispatch)?;
//           }
//         }
//       }
//       "emit_signal" => {
//         let name = payload.get::<String>("name")?;
//         let flow_payload = FlowPayload::from_json(payload.0.get("payload").cloned());

//         let def = ctx
//           .registry
//           .metadata
//           .lookup::<Signal>("units", &name)
//           .ok_or_else(|| CoreError::InvalidState(format!("signal not found: {name}")))?;

//         if !Self::payload_type_ok(def.payload, &flow_payload) {
//           return Err(CoreError::InvalidState(format!(
//             "signal payload type mismatch: {name}"
//           )));
//         }

//         let instance = FlowInstance {
//           name,
//           payload: flow_payload,
//           r#type: FlowType::Signal,
//         };

//         self.dispatch_service_transitions(&instance, true, ctx, dispatch)?;
//       }
//       _ => {}
//     }

//     Ok(())
//   }
// }

// pub struct UnitsOrchestrator {
//   units_dir: PathBuf,
// }

// impl UnitsOrchestrator {
//   pub fn new(units_dir: impl Into<PathBuf>) -> Self {
//     Self {
//       units_dir: units_dir.into(),
//     }
//   }
// }

// impl Orchestrator for UnitsOrchestrator {
//   fn id(&self) -> &str {
//     UNITS_ORCHESTRATOR_ID
//   }

//   fn depends_on(&self) -> &[String] {
//     &[]
//   }

//   fn when(&self) -> OrchestratorWhen<'static> {
//     OrchestratorWhen {
//       cycle: &[BootCycle::Collect],
//       phase: BootPhase::Start,
//     }
//   }

//   fn preload(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
//     if ctx.metadata.metadata("units").is_none() {
//       ctx.metadata.insert_metadata(
//         Metadata::new("units")
//           .of::<Service>("service")
//           .of::<State>("state")
//           .of::<Signal>("signal"),
//       );
//     }

//     for (group, source) in read_units(self.units_dir.as_path())? {
//       ctx
//         .metadata
//         .load_group_from_toml("units", &group, &source)
//         .map_err(|e| CoreError::InvalidState(e.to_string()))?;
//     }

//     Ok(())
//   }

//   fn run(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
//     Ok(())
//   }
// }

// pub struct FlowOrchestrator {
//   depends_on: Vec<String>,
// }

// impl Default for FlowOrchestrator {
//   fn default() -> Self {
//     Self {
//       depends_on: vec![UNITS_ORCHESTRATOR_ID.to_string()],
//     }
//   }
// }

// impl Orchestrator for FlowOrchestrator {
//   fn id(&self) -> &str {
//     FLOW_ORCHESTRATOR_ID
//   }

//   fn depends_on(&self) -> &[String] {
//     self.depends_on.as_slice()
//   }

//   fn when(&self) -> OrchestratorWhen<'static> {
//     OrchestratorWhen {
//       cycle: &[BootCycle::Runtime],
//       phase: BootPhase::Start,
//     }
//   }

//   fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<(), CoreError> {
//     ctx.dispatch(FLOW_RUNTIME_ID, "bootstrap", serde_json::json!({}))
//   }
// }

// pub fn install_units(boot: &mut BootEngine, units_dir: impl Into<PathBuf>) {
//   boot.orchestrators.push(UnitsOrchestrator::new(units_dir));
// }

// pub fn install_flow(boot: &mut BootEngine) {
//   boot.orchestrators.push(FlowOrchestrator::default());
// }

// fn read_units(path: &Path) -> Result<Vec<(String, String)>, CoreError> {
//   let entries = fs::read_dir(path).map_err(|err| {
//     CoreError::InvalidState(format!(
//       "failed to read units dir `{}`: {err}",
//       path.display()
//     ))
//   })?;

//   let mut out = Vec::new();
//   for entry in entries.flatten() {
//     let file_path = entry.path();
//     if file_path.extension().and_then(|x| x.to_str()) != Some("toml") {
//       continue;
//     }

//     let Some(stem) = file_path.file_stem().and_then(|x| x.to_str()) else {
//       continue;
//     };

//     let source = fs::read_to_string(file_path.as_path()).map_err(|err| {
//       CoreError::InvalidState(format!(
//         "failed to read unit file `{}`: {err}",
//         file_path.display()
//       ))
//     })?;
//     out.push((stem.to_string(), source));
//   }

//   Ok(out)
// }

// fn subset_match(filter: &serde_json::Value, payload: &serde_json::Value) -> bool {
//   match (filter, payload) {
//     (serde_json::Value::Object(f_tab), serde_json::Value::Object(p_tab)) => {
//       for (key, f_val) in f_tab {
//         let Some(p_val) = p_tab.get(key) else {
//           return false;
//         };
//         if !subset_match(f_val, p_val) {
//           return false;
//         }
//       }
//       true
//     }
//     (serde_json::Value::Array(f_arr), serde_json::Value::Array(p_arr)) => {
//       for f_val in f_arr {
//         if !p_arr.iter().any(|p_val| subset_match(f_val, p_val)) {
//           return false;
//         }
//       }
//       true
//     }
//     (f, p) => f == p,
//   }
// }
