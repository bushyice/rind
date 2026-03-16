use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use rind_core::prelude::*;

use crate::transport::TransportMethod;
use crate::triggers::{
  branch_target_key, check_condition, default_payload_for_type, json_branch_key, map_json_payload,
  merge_json, payload_compatible, payload_signature, payload_to_filter, run_eval,
};

pub const FLOW_RUNTIME_ID: &str = "flow";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowItem {
  Simple(String),
  Detailed {
    state: Option<String>,
    signal: Option<String>,
    target: Option<FlowMatchOperation>,
    branch: Option<FlowMatchOperation>,
  },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowMatchOperation {
  Eq(String),
  Options {
    binary: Option<bool>,
    contains: Option<String>,
    r#as: Option<serde_json::Value>,
  },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Trigger {
  pub script: Option<String>,
  pub exec: Option<String>,
  pub args: Option<Vec<String>>,
  pub state: Option<String>,
  pub signal: Option<String>,
  pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowType {
  #[default]
  Signal,
  State,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlowJson(pub String);

impl From<String> for FlowJson {
  fn from(value: String) -> Self {
    Self(value)
  }
}

impl FlowJson {
  pub fn into_json(&self) -> serde_json::Value {
    serde_json::from_str(&self.0).unwrap_or(serde_json::Value::Null)
  }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FlowPayload {
  Json(FlowJson),
  String(String),
  Bytes(Vec<u8>),
  None(bool),
}

impl FlowPayload {
  pub fn to_string_payload(&self) -> String {
    match self {
      FlowPayload::Json(v) => v.0.clone(),
      FlowPayload::String(v) => v.clone(),
      FlowPayload::Bytes(v) => String::from_utf8(v.clone()).unwrap_or_default(),
      FlowPayload::None(_) => String::new(),
    }
  }

  pub fn to_json(&self) -> serde_json::Value {
    match self {
      FlowPayload::Json(v) => v.into_json(),
      FlowPayload::String(v) => serde_json::Value::String(v.clone()),
      FlowPayload::Bytes(v) => serde_json::json!(v),
      FlowPayload::None(_) => serde_json::Value::Null,
    }
  }

  pub fn from_json(v: Option<serde_json::Value>) -> Self {
    match v {
      Some(serde_json::Value::Object(v)) => {
        FlowPayload::Json(FlowJson(serde_json::Value::Object(v).to_string()))
      }
      Some(serde_json::Value::Array(v)) => {
        FlowPayload::Json(FlowJson(serde_json::Value::Array(v).to_string()))
      }
      Some(serde_json::Value::String(v)) => FlowPayload::String(v),
      Some(serde_json::Value::Null) | None => FlowPayload::None(false),
      Some(v) => FlowPayload::String(v.to_string()),
    }
  }

  pub fn get_json_field(&self, field: &str) -> Option<serde_json::Value> {
    match self {
      FlowPayload::Json(s) => s.into_json().get(field).cloned(),
      _ => None,
    }
  }

  pub fn contains(&self, needle: &str) -> bool {
    match self {
      FlowPayload::String(s) => s.contains(needle),
      FlowPayload::Json(s) => s.0.contains(needle),
      _ => false,
    }
  }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FlowInstance {
  pub name: String,
  pub payload: FlowPayload,
  pub r#type: FlowType,
}

impl From<StateEntry> for FlowInstance {
  fn from(value: StateEntry) -> Self {
    let cfg = bincode_next::config::standard();
    if let Ok((instance, _)) =
      bincode_next::serde::decode_from_slice::<FlowInstance, _>(&value.data, cfg)
    {
      return instance;
    }
    FlowInstance {
      name: String::new(),
      payload: FlowPayload::None(false),
      r#type: FlowType::State,
    }
  }
}

impl From<&FlowInstance> for StateEntry {
  fn from(value: &FlowInstance) -> Self {
    let cfg = bincode_next::config::standard();
    let data = bincode_next::serde::encode_to_vec(value, cfg).unwrap_or_default();
    StateEntry { data }
  }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowPayloadType {
  #[default]
  Json,
  String,
  Bytes,
  None,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum AutoPayloadInsert {
  One(String),
  Many(Vec<String>),
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AutoPayloadConfig {
  pub eval: Option<String>,
  pub args: Option<Vec<String>>,
  pub insert: Option<AutoPayloadInsert>,
}

#[model(
  meta_name = name,
  meta_fields(
    name, payload, activate_on_none, after, branch, auto_payload, subscribers, broadcast
  ),
  derive_metadata(Debug, Clone)
)]
pub struct State {
  pub name: String,
  pub payload: FlowPayloadType,
  #[serde(rename = "activate-on-none")]
  pub activate_on_none: Option<Vec<String>>,
  pub after: Option<Vec<FlowItem>>,
  pub branch: Option<Vec<String>>,
  #[serde(rename = "auto-payload")]
  pub auto_payload: Option<AutoPayloadConfig>,
  pub subscribers: Option<Vec<TransportMethod>>,
  pub broadcast: Option<Vec<String>>,
}

#[model(
  meta_name = name,
  meta_fields(name, payload, subscribers, broadcast),
  derive_metadata(Debug, Clone)
)]
pub struct Signal {
  pub name: String,
  pub payload: FlowPayloadType,
  pub subscribers: Option<Vec<TransportMethod>>,
  pub broadcast: Option<Vec<String>>,
}

#[derive(Default)]
pub struct StateMachine {
  pub states: HashMap<String, Vec<FlowInstance>>,
}

pub type StateMachineShared = Arc<RwLock<StateMachine>>;

impl StateMachine {
  pub fn load_from_persistence(&mut self, persistence: StateSnapshot) {
    self.states = persistence
      .into_iter()
      .map(|(name, i)| {
        (
          name.clone(),
          i.into_iter()
            .map(FlowInstance::from)
            .filter(|x| !x.name.is_empty())
            .collect(),
        )
      })
      .collect();
  }

  pub fn snapshot_for_persistence(&self) -> StateSnapshot {
    self
      .states
      .iter()
      .map(|(name, states)| {
        (
          name.clone(),
          states.iter().map(StateEntry::from).collect::<Vec<_>>(),
        )
      })
      .collect()
  }
}

#[derive(Default)]
pub struct FlowRuntime;

impl FlowRuntime {
  fn transport_id(subscriber: &TransportMethod) -> &str {
    match subscriber {
      TransportMethod::Type(id) => id.0.as_str(),
      TransportMethod::Options { id, .. } => id.0.as_str(),
      TransportMethod::Object { id, .. } => id.0.as_str(),
    }
  }

  fn setup_subscriber_endpoint(
    dispatch: &RuntimeDispatcher,
    endpoint: &str,
    subscriber: &TransportMethod,
  ) {
    if Self::transport_id(subscriber) == "uds" {
      let _ = dispatch.dispatch(
        "transport",
        "setup_uds",
        serde_json::json!({ "endpoint": endpoint }).into(),
      );
    }
  }

  fn publish_to_state_subscribers(
    dispatch: &RuntimeDispatcher,
    endpoint: &str,
    payload: &FlowPayload,
    action: FlowAction,
    subscribers: Option<&[TransportMethod]>,
  ) {
    let Some(subscribers) = subscribers else {
      return;
    };

    for subscriber in subscribers {
      Self::setup_subscriber_endpoint(dispatch, endpoint, subscriber);
      let action = match action {
        FlowAction::Apply => "set",
        FlowAction::Revert => "remove",
      };
      let _ = dispatch.dispatch(
        "transport",
        "send",
        serde_json::json!({
          "endpoint": endpoint,
          "type": "state",
          "name": endpoint,
          "action": action,
          "payload": payload.to_json(),
        })
        .into(),
      );
    }
  }

  fn setup_all_state_subscribers(
    dispatch: &RuntimeDispatcher,
    state_defs: &[(String, Arc<StateMetadata>)],
  ) {
    for (name, def) in state_defs {
      if let Some(subscribers) = def.subscribers.as_deref() {
        for subscriber in subscribers {
          Self::setup_subscriber_endpoint(dispatch, name, subscriber);
        }
      }
    }
  }

  fn state_subscribers_for<'a>(
    name: &str,
    state_defs: &'a [(String, Arc<StateMetadata>)],
  ) -> Option<&'a [TransportMethod]> {
    state_defs
      .iter()
      .find(|(full_name, _)| full_name == name)
      .or_else(|| {
        let item_name = name.split_once('@').map(|(_, n)| n).unwrap_or(name);
        state_defs.iter().find(|(_, d)| d.name == item_name)
      })
      .and_then(|(_, d)| d.subscribers.as_deref())
  }

  fn save_state_machine(
    sm: &StateMachine,
    persistence: Option<&Arc<RwLock<StatePersistence>>>,
  ) -> Result<(), CoreError> {
    let Some(persistence) = persistence else {
      return Ok(());
    };
    let snapshot = sm.snapshot_for_persistence();
    persistence
      .write()
      .map_err(CoreError::custom)?
      .save(snapshot);
    Ok(())
  }

  fn payload_type_ok(expected: FlowPayloadType, payload: &FlowPayload) -> bool {
    match payload {
      FlowPayload::Json(_) => expected == FlowPayloadType::Json,
      FlowPayload::String(_) => expected == FlowPayloadType::String,
      FlowPayload::Bytes(_) => expected == FlowPayloadType::Bytes,
      FlowPayload::None(_) => expected == FlowPayloadType::None,
    }
  }

  fn set_state(
    sm: &mut StateMachine,
    name: String,
    payload: Option<FlowPayload>,
    state_defs: &[(String, Arc<StateMetadata>)],
    signal_defs: &[(String, Arc<SignalMetadata>)],
    guard: &mut HashSet<String>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<(), CoreError> {
    let branch_sig = payload_signature(&payload);
    let guard_key = format!("apply::{name}::{branch_sig}");
    if guard.contains(&guard_key) {
      return Ok(());
    }
    guard.insert(guard_key.clone());

    let def = state_defs
      .iter()
      .find(|(full_name, _)| *full_name == name)
      .or_else(|| {
        let item_name = name.split_once('@').map(|(_, n)| n).unwrap_or(&name);
        state_defs.iter().find(|(_, d)| d.name == item_name)
      })
      .map(|(_, d)| d.clone())
      .ok_or_else(|| CoreError::InvalidState(format!("state not found: {name}")))?;

    let flow_payload = payload.unwrap_or(FlowPayload::None(false));
    if !Self::payload_type_ok(def.payload, &flow_payload) {
      guard.remove(&guard_key);
      return Err(CoreError::InvalidState(format!(
        "state payload type mismatch: {name}"
      )));
    }

    let branches = def.branch.clone();

    let instance = FlowInstance {
      name: name.clone(),
      payload: flow_payload,
      r#type: FlowType::State,
    };

    event_bus.emit(FlowEvent {
      name: name.clone(),
      payload: instance.payload.to_json(),
      action: FlowAction::Apply,
      flow_type: FlowEventType::State,
    });
    Self::publish_to_state_subscribers(
      dispatch,
      name.as_str(),
      &instance.payload,
      FlowAction::Apply,
      def.subscribers.as_deref(),
    );

    let entry = sm.states.entry(name.clone()).or_default();
    match &instance.payload {
      FlowPayload::String(_) | FlowPayload::Bytes(_) | FlowPayload::None(_) => {
        entry.clear();
        entry.push(instance.clone());
      }
      FlowPayload::Json(new_json) => {
        let branch_keys = branches
          .as_ref()
          .map(|b| {
            b.iter()
              .map(|key| branch_target_key(key.as_str()).to_string())
              .collect::<Vec<String>>()
          })
          .unwrap_or_else(|| vec!["id".to_string()]);

        let new_key = json_branch_key(&new_json.into_json(), &branch_keys).ok_or_else(|| {
          guard.remove(&guard_key);
          CoreError::InvalidState("invalid JSON branch keys".into())
        })?;

        let mut found = false;
        for branch in entry.iter_mut() {
          if let FlowPayload::Json(json) = &mut branch.payload {
            let mut existing_json = json.into_json();
            let existing_key = json_branch_key(&existing_json, &branch_keys);
            if existing_key == Some(new_key.clone()) {
              merge_json(&mut existing_json, &new_json.into_json());
              *json = FlowJson(existing_json.to_string());
              found = true;
              break;
            }
          }
        }

        if !found {
          entry.push(instance.clone());
        }
      }
    }

    Self::reconcile_transcendence(
      sm,
      &instance,
      FlowAction::Apply,
      state_defs,
      signal_defs,
      guard,
      event_bus,
      dispatch,
    );
    Self::reconcile_activate_on_none(sm, state_defs, signal_defs, guard, event_bus, dispatch);

    guard.remove(&guard_key);
    Ok(())
  }

  fn remove_state(
    sm: &mut StateMachine,
    name: &str,
    filter: Option<FlowMatchOperation>,
    state_defs: &[(String, Arc<StateMetadata>)],
    signal_defs: &[(String, Arc<SignalMetadata>)],
    guard: &mut HashSet<String>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) {
    if let Some(branches) = sm.states.remove(name) {
      let (to_keep, to_remove): (Vec<_>, Vec<_>) = if let Some(filter) = &filter {
        branches
          .into_iter()
          .partition(|b| !crate::triggers::match_operation(filter, &b.payload))
      } else {
        (Vec::new(), branches)
      };

      for mut branch in to_remove {
        branch.r#type = FlowType::State;
        let guard_key = format!(
          "revert::{}::{}",
          branch.name,
          payload_signature(&Some(branch.payload.clone()))
        );
        if guard.contains(&guard_key) {
          continue;
        }
        guard.insert(guard_key.clone());

        event_bus.emit(FlowEvent {
          name: branch.name.clone(),
          payload: branch.payload.to_json(),
          action: FlowAction::Revert,
          flow_type: FlowEventType::State,
        });
        let subscribers = Self::state_subscribers_for(name, state_defs);
        Self::publish_to_state_subscribers(
          dispatch,
          branch.name.as_str(),
          &branch.payload,
          FlowAction::Revert,
          subscribers,
        );

        Self::reconcile_transcendence(
          sm,
          &branch,
          FlowAction::Revert,
          state_defs,
          signal_defs,
          guard,
          event_bus,
          dispatch,
        );
        Self::reconcile_activate_on_none(sm, state_defs, signal_defs, guard, event_bus, dispatch);
        guard.remove(&guard_key);
      }

      if !to_keep.is_empty() {
        sm.states.insert(name.to_string(), to_keep);
      }
    }
  }

  fn emit_signal(
    name: String,
    payload: Option<FlowPayload>,
    signal_defs: &[(String, Arc<SignalMetadata>)],
    event_bus: &EventBus,
  ) -> Result<(), CoreError> {
    let def = signal_defs
      .iter()
      .find(|(full_name, _)| *full_name == name)
      .or_else(|| {
        let item_name = name.split_once('@').map(|(_, n)| n).unwrap_or(&name);
        signal_defs.iter().find(|(_, d)| d.name == item_name)
      })
      .map(|(_, d)| d.clone())
      .ok_or_else(|| CoreError::InvalidState(format!("signal not found: {name}")))?;

    let flow_payload = payload.unwrap_or(FlowPayload::None(false));
    if !Self::payload_type_ok(def.payload, &flow_payload) {
      return Err(CoreError::InvalidState(format!(
        "signal payload type mismatch: {name}"
      )));
    }

    event_bus.emit(FlowEvent {
      name: name.clone(),
      payload: flow_payload.to_json(),
      action: FlowAction::Apply,
      flow_type: FlowEventType::Signal,
    });

    Ok(())
  }

  fn reconcile_transcendence(
    sm: &mut StateMachine,
    source: &FlowInstance,
    action: FlowAction,
    state_defs: &[(String, Arc<StateMetadata>)],
    signal_defs: &[(String, Arc<SignalMetadata>)],
    guard: &mut HashSet<String>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) {
    let dependents: Vec<(String, FlowPayload)> = state_defs
      .iter()
      .filter_map(|(full_name, def)| {
        let after = def.after.as_ref()?;
        if !after.iter().any(|cond| check_condition(cond, source)) {
          return None;
        }

        let source_payload = if def.auto_payload.is_some() {
          &auto_payload_for(def, Some(&source.payload))
        } else {
          &source.payload
        };

        let payload = transcendent_payload_for(def, source_payload)?;

        let all_active = after
          .iter()
          .all(|cond| condition_is_active(sm, cond, Some(&payload)));

        match action {
          FlowAction::Apply if !all_active => None,
          FlowAction::Revert if all_active => None,
          _ => Some((full_name.clone(), payload)),
        }
      })
      .filter(|(name, _)| *name != source.name)
      .collect();

    for (dependent, payload) in dependents {
      match action {
        FlowAction::Apply => {
          let _ = Self::set_state(
            sm,
            dependent,
            Some(payload),
            state_defs,
            signal_defs,
            guard,
            event_bus,
            dispatch,
          );
        }
        FlowAction::Revert => {
          Self::remove_state(
            sm,
            &dependent,
            payload_to_filter(&payload),
            state_defs,
            signal_defs,
            guard,
            event_bus,
            dispatch,
          );
        }
      }
    }
  }

  fn reconcile_activate_on_none(
    sm: &mut StateMachine,
    state_defs: &[(String, Arc<StateMetadata>)],
    signal_defs: &[(String, Arc<SignalMetadata>)],
    guard: &mut HashSet<String>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) {
    let targets: Vec<(String, bool, FlowPayload)> = state_defs
      .iter()
      .filter_map(|(full_name, def)| {
        let deps = def.activate_on_none.as_ref()?;
        let should_activate = deps
          .iter()
          .all(|name| sm.states.get(name).map(|v| v.is_empty()).unwrap_or(true));
        Some((
          full_name.clone(),
          should_activate,
          auto_payload_for(def, None),
        ))
      })
      .collect();

    for (name, should_activate, payload) in targets {
      let currently_active = sm.states.get(&name).map(|v| !v.is_empty()).unwrap_or(false);

      if should_activate && !currently_active {
        let _ = Self::set_state(
          sm,
          name,
          Some(payload),
          state_defs,
          signal_defs,
          guard,
          event_bus,
          dispatch,
        );
      } else if !should_activate && currently_active {
        Self::remove_state(
          sm,
          &name,
          None,
          state_defs,
          signal_defs,
          guard,
          event_bus,
          dispatch,
        );
      }
    }
  }

  fn collect_defs(
    metadata: &MetadataRegistry,
  ) -> (
    Vec<(String, Arc<StateMetadata>)>,
    Vec<(String, Arc<SignalMetadata>)>,
  ) {
    let mut state_defs = Vec::new();
    let mut signal_defs = Vec::new();

    if let Some(m) = metadata.metadata("units") {
      for group in m.groups() {
        if let Some(states) = metadata.group_items::<State>("units", group) {
          for s in states {
            state_defs.push((format!("{group}@{}", s.name), s));
          }
        }
        if let Some(signals) = metadata.group_items::<Signal>("units", group) {
          for s in signals {
            signal_defs.push((format!("{group}@{}", s.name), s));
          }
        }
      }
    }

    (state_defs, signal_defs)
  }
}

impl Runtime for FlowRuntime {
  fn id(&self) -> &str {
    FLOW_RUNTIME_ID
  }

  fn handle(
    &mut self,
    action: &str,
    payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    _log: &LogHandle,
  ) -> Result<(), CoreError> {
    let sm_shared = ctx
      .scope
      .get::<StateMachineShared>()
      .cloned()
      .ok_or_else(|| CoreError::InvalidState("state machine not found in scope".into()))?;
    let persistence = ctx.scope.get::<Arc<RwLock<StatePersistence>>>().cloned();

    let event_bus = ctx.scope.get::<EventBus>().cloned().unwrap_or_default();

    let (state_defs, signal_defs) = Self::collect_defs(ctx.registry.metadata);

    match action {
      "set_state" => {
        let name = payload.get::<String>("name")?;
        let flow_payload = FlowPayload::from_json(payload.0.get("payload").cloned());

        let mut sm = sm_shared
          .write()
          .map_err(|e| CoreError::InvalidState(format!("state machine lock failed: {e}")))?;
        let mut guard = HashSet::new();
        Self::set_state(
          &mut sm,
          name,
          Some(flow_payload),
          &state_defs,
          &signal_defs,
          &mut guard,
          &event_bus,
          dispatch,
        )?;
        Self::save_state_machine(&sm, persistence.as_ref())?;
      }
      "remove_state" => {
        let name = payload.get::<String>("name")?;
        let filter_json: Option<serde_json::Value> = payload.0.get("filter").cloned();
        let filter = filter_json.and_then(|v| serde_json::from_value(v).ok());

        let mut sm = sm_shared
          .write()
          .map_err(|e| CoreError::InvalidState(format!("state machine lock failed: {e}")))?;
        let mut guard = HashSet::new();
        Self::remove_state(
          &mut sm,
          &name,
          filter,
          &state_defs,
          &signal_defs,
          &mut guard,
          &event_bus,
          dispatch,
        );
        Self::save_state_machine(&sm, persistence.as_ref())?;
      }
      "emit_signal" => {
        let name = payload.get::<String>("name")?;
        let flow_payload = FlowPayload::from_json(payload.0.get("payload").cloned());
        Self::emit_signal(name, Some(flow_payload), &signal_defs, &event_bus)?;
      }
      "bootstrap" => {
        Self::setup_all_state_subscribers(dispatch, &state_defs);

        let mut sm = sm_shared
          .write()
          .map_err(|e| CoreError::InvalidState(format!("state machine lock failed: {e}")))?;
        let mut guard = HashSet::new();

        let existing_states = sm
          .states
          .values()
          .flat_map(|branches| branches.iter().cloned())
          .collect::<Vec<_>>();

        for state in &existing_states {
          Self::reconcile_transcendence(
            &mut sm,
            state,
            FlowAction::Apply,
            &state_defs,
            &signal_defs,
            &mut guard,
            &event_bus,
            dispatch,
          );
        }

        Self::reconcile_activate_on_none(
          &mut sm,
          &state_defs,
          &signal_defs,
          &mut guard,
          &event_bus,
          dispatch,
        );
        Self::save_state_machine(&sm, persistence.as_ref())?;
      }
      _ => {}
    }

    Ok(())
  }
}

pub fn condition_is_active(
  sm: &StateMachine,
  cond: &FlowItem,
  payload: Option<&FlowPayload>,
) -> bool {
  for branches in sm.states.values() {
    for branch in branches {
      let mut state = branch.clone();
      state.r#type = FlowType::State;
      if check_condition(cond, &state) && payload_compatible(payload, &state.payload) {
        return true;
      }
    }
  }
  false
}

fn auto_payload_for(def: &StateMetadata, _payload: Option<&FlowPayload>) -> FlowPayload {
  let Some(cfg) = &def.auto_payload else {
    return default_payload_for_type(def.payload);
  };

  let output = if let Some(eval) = &cfg.eval {
    run_eval(eval.as_str(), cfg.args.clone())
  } else {
    String::new()
  };

  let lines: Vec<String> = output
    .lines()
    .map(|x| x.trim().to_string())
    .filter(|x| !x.is_empty())
    .collect();

  match def.payload {
    FlowPayloadType::Json => {
      let mut obj = serde_json::Map::new();
      match &cfg.insert {
        Some(AutoPayloadInsert::One(key)) if key == "root" => {
          if lines.len() == 1 {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&lines[0]) {
              return FlowPayload::Json(v.to_string().into());
            }
            return FlowPayload::Json(
              serde_json::Value::String(lines[0].clone())
                .to_string()
                .into(),
            );
          }
          return FlowPayload::Json(
            serde_json::to_value(&lines)
              .unwrap_or_default()
              .to_string()
              .into(),
          );
        }
        Some(AutoPayloadInsert::One(key)) => {
          if let Some(first) = lines.first() {
            obj.insert(key.clone(), serde_json::Value::String(first.clone()));
          }
        }
        Some(AutoPayloadInsert::Many(keys)) => {
          for (i, key) in keys.iter().enumerate() {
            if let Some(line) = lines.get(i) {
              obj.insert(key.clone(), serde_json::Value::String(line.clone()));
            }
          }
        }
        None => {
          if let Some(first) = lines.first() {
            obj.insert(
              "value".to_string(),
              serde_json::Value::String(first.clone()),
            );
          }
        }
      }
      FlowPayload::Json(serde_json::Value::Object(obj).to_string().into())
    }
    FlowPayloadType::String => FlowPayload::String(lines.join("\n")),
    FlowPayloadType::Bytes => FlowPayload::Bytes(output.into_bytes()),
    FlowPayloadType::None => FlowPayload::None(false),
  }
}

fn transcendent_payload_for(
  def: &StateMetadata,
  source_payload: &FlowPayload,
) -> Option<FlowPayload> {
  match def.payload {
    FlowPayloadType::Json => {
      if let Some(branch_specs) = &def.branch {
        map_json_payload(branch_specs, source_payload)
      } else if matches!(source_payload, FlowPayload::Json(_)) {
        Some(source_payload.clone())
      } else {
        None
      }
    }
    FlowPayloadType::String => {
      if matches!(source_payload, FlowPayload::String(_)) {
        Some(source_payload.clone())
      } else {
        None
      }
    }
    FlowPayloadType::Bytes => {
      if matches!(source_payload, FlowPayload::Bytes(_)) {
        Some(source_payload.clone())
      } else {
        None
      }
    }
    FlowPayloadType::None => Some(FlowPayload::None(false)),
  }
}
