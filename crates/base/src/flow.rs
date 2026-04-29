use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

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
  Simple(Ustr),
  Detailed {
    state: Option<Ustr>,
    signal: Option<Ustr>,
    target: Option<FlowMatchOperation>,
    branch: Option<FlowMatchOperation>,
  },
}

impl FlowItem {
  pub fn name(&self) -> &Ustr {
    match self {
      FlowItem::Simple(s) => s,
      FlowItem::Detailed {
        state,
        signal,
        target: _,
        branch: _,
      } => {
        if let Some(state) = state {
          state
        } else {
          signal.as_ref().unwrap()
        }
      }
    }
  }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowMatchOperation {
  Eq(Ustr),
  Options {
    binary: Option<bool>,
    contains: Option<Ustr>,
    r#as: Option<serde_json::Value>,
  },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Trigger {
  pub script: Option<Ustr>,
  pub exec: Option<Ustr>,
  pub args: Option<Vec<Ustr>>,
  pub state: Option<Ustr>,
  pub signal: Option<Ustr>,
  pub service: Option<Ustr>,
  pub timer: Option<Ustr>,
  pub socket: Option<Ustr>,
  pub stop: Option<bool>,
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

  pub fn set_json(&mut self, key: String, value: serde_json::Value) {
    match self {
      FlowPayload::Json(v) => {
        let mut json = v.into_json();
        merge_json(&mut json, &serde_json::json!({ key: value }));
        v.0 = json.to_string();
      }
      _ => {}
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

  pub fn get_json_field_as<T: serde::de::DeserializeOwned>(&self, field: &str) -> Option<T> {
    match self {
      FlowPayload::Json(s) => serde_json::from_value(s.into_json().get(field).cloned()?).ok(),
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
  pub name: Ustr,
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
      name: Ustr::from(""),
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
    name, payload, activate_on_none, after, branch, auto_payload, subscribers, broadcast, permissions
  ),
  derive_metadata(Debug, Clone)
)]
pub struct State {
  pub name: Ustr,
  pub payload: FlowPayloadType,
  #[serde(rename = "activate-on-none")]
  pub activate_on_none: Option<Vec<Ustr>>,
  pub after: Option<Vec<FlowItem>>,
  pub branch: Option<Vec<Ustr>>,
  #[serde(rename = "auto-payload")]
  pub auto_payload: Option<AutoPayloadConfig>,
  pub subscribers: Option<Vec<TransportMethod>>,
  pub broadcast: Option<Vec<Ustr>>,
  pub permissions: Option<Vec<Ustr>>,
}

#[model(
  meta_name = name,
  meta_fields(name, payload, after, branch, subscribers, broadcast, permissions),
  derive_metadata(Debug, Clone)
)]
pub struct Signal {
  pub name: Ustr,
  pub payload: FlowPayloadType,
  pub after: Option<Vec<FlowItem>>,
  pub branch: Option<Vec<Ustr>>,
  pub subscribers: Option<Vec<TransportMethod>>,
  pub broadcast: Option<Vec<Ustr>>,
  pub permissions: Option<Vec<Ustr>>,
}

#[derive(Clone)]
pub struct StateMachine {
  pub states: HashMap<Ustr, Vec<FlowInstance>>,
  persistence: StatePersistence,
}

impl StateMachine {
  pub const KEY: &str = "runtime@state_machine";

  pub fn from_persistence(persistence: StatePersistence) -> Self {
    Self {
      persistence: persistence,
      states: Default::default(),
    }
  }

  pub fn load_from_persistence(&mut self) -> Result<(), CoreError> {
    self.states = self
      .persistence
      .load()?
      .into_iter()
      .map(|(name, i)| {
        (
          Ustr::from(name),
          i.into_iter()
            .map(FlowInstance::from)
            .filter(|x| !x.name.as_str().is_empty())
            .collect(),
        )
      })
      .collect();
    Ok(())
  }

  pub fn snapshot_for_persistence(&self) -> StateSnapshot {
    self
      .states
      .iter()
      .filter_map(|(name, states)| {
        // State impermanence
        if name.as_str().contains("@_") {
          return None;
        }
        Some((
          name.to_string(),
          states.iter().map(StateEntry::from).collect::<Vec<_>>(),
        ))
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
      // println!("{:?} {:?}", subscriber, subscriber.get_permissions());
      let payload = RuntimePayload::default().insert("endpoint", endpoint.to_ustr());
      let _ = dispatch.dispatch(
        "transport",
        "setup_uds",
        if let Some(perms) = subscriber.get_permissions() {
          payload.insert("permissions", perms)
        } else {
          payload
        },
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
        RuntimePayload::default()
          .insert("endpoint", endpoint.to_ustr())
          .insert("type", "state".to_string())
          .insert("name", endpoint.to_ustr())
          .insert("action", action.to_string())
          .insert("payload", payload.to_json()),
      );
    }
  }

  fn setup_all_state_subscribers(
    dispatch: &RuntimeDispatcher,
    state_defs: &[(Ustr, Arc<StateMetadata>)],
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
    state_defs: &'a [(Ustr, Arc<StateMetadata>)],
  ) -> Option<&'a [TransportMethod]> {
    state_defs
      .iter()
      .find(|(full_name, _)| full_name.as_str() == name)
      .or_else(|| {
        let item_name = name.split_once('@').map(|(_, n)| n).unwrap_or(name);
        state_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
      })
      .and_then(|(_, d)| d.subscribers.as_deref())
  }

  fn save_state_machine(sm: &StateMachine) -> Result<(), CoreError> {
    let snapshot = sm.snapshot_for_persistence();
    sm.persistence.save(snapshot);
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
    name: impl Into<Ustr>,
    payload: Option<FlowPayload>,
    state_defs: &[(Ustr, Arc<StateMetadata>)],
    signal_defs: &[(Ustr, Arc<SignalMetadata>)],
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<(), CoreError> {
    let name = name.into();
    let branch_sig = payload_signature(&payload);
    let guard_key = Ustr::from(format!("apply::{name}::{branch_sig}"));
    if guard.contains(&guard_key) {
      return Ok(());
    }
    guard.insert(guard_key.clone());

    let def = state_defs
      .iter()
      .find(|(full_name, _)| *full_name == name)
      .or_else(|| {
        let item_name = name
          .as_str()
          .split_once('@')
          .map(|(_, n)| n)
          .unwrap_or(name.as_str());
        state_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
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

    let entry = sm.states.entry(name).or_default();
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
              .map(|key| Ustr::from(branch_target_key(key.as_str())))
              .collect::<Vec<Ustr>>()
          })
          .unwrap_or_else(|| vec!["id".into()]);

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
    state_defs: &[(Ustr, Arc<StateMetadata>)],
    signal_defs: &[(Ustr, Arc<SignalMetadata>)],
    guard: &mut HashSet<Ustr>,
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
        let guard_key = Ustr::from(format!(
          "revert::{}::{}",
          branch.name,
          payload_signature(&Some(branch.payload.clone()))
        ));
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
        sm.states.insert(Ustr::from(name.to_string()), to_keep);
      }
    }
  }

  fn emit_signal(
    name: impl Into<Ustr>,
    payload: Option<FlowPayload>,
    signal_defs: &[(Ustr, Arc<SignalMetadata>)],
    event_bus: &EventBus,
  ) -> Result<(), CoreError> {
    let name = name.into();
    let def = signal_defs
      .iter()
      .find(|(full_name, _)| *full_name == name)
      .or_else(|| {
        let item_name = name
          .as_str()
          .split_once('@')
          .map(|(_, n)| n)
          .unwrap_or(name.as_str());
        signal_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
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

  fn reconcile_signal_transcendence(
    sm: &StateMachine,
    source: &FlowInstance,
    signal_defs: &[(Ustr, Arc<SignalMetadata>)],
    event_bus: &EventBus,
    emitted: &mut HashSet<Ustr>,
  ) {
    let dependents: Vec<(Ustr, FlowPayload)> = signal_defs
      .iter()
      .filter_map(|(full_name, def)| {
        let after = def.after.as_ref()?;
        if !after.iter().any(|cond| check_condition(cond, source)) {
          return None;
        }
        let all_active = after
          .iter()
          .all(|cond| condition_matches(sm, cond, Some(source), None));
        if !all_active {
          return None;
        }
        let payload = if let Some(branch_specs) = &def.branch {
          map_json_payload(branch_specs, &source.payload)?
        } else {
          source.payload.clone()
        };
        Some((full_name.clone(), payload))
      })
      .collect();

    for (signal_name, payload) in dependents {
      let sig = Ustr::from(format!("{signal_name}|{}", payload.to_string_payload()));
      if emitted.contains(&sig) {
        continue;
      }
      emitted.insert(sig);
      let _ = Self::emit_signal(signal_name, Some(payload), signal_defs, event_bus);
    }
  }

  fn reconcile_transcendence(
    sm: &mut StateMachine,
    source: &FlowInstance,
    action: FlowAction,
    state_defs: &[(Ustr, Arc<StateMetadata>)],
    signal_defs: &[(Ustr, Arc<SignalMetadata>)],
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) {
    let dependents: Vec<(Ustr, FlowPayload)> = state_defs
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
          .all(|cond| condition_matches(sm, cond, Some(source), Some(&payload)));

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
    state_defs: &[(Ustr, Arc<StateMetadata>)],
    signal_defs: &[(Ustr, Arc<SignalMetadata>)],
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) {
    let targets: Vec<(Ustr, bool, FlowPayload)> = state_defs
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
    Vec<(Ustr, Arc<StateMetadata>)>,
    Vec<(Ustr, Arc<SignalMetadata>)>,
  ) {
    let mut state_defs = Vec::new();
    let mut signal_defs = Vec::new();

    if let Some(m) = metadata.metadata("units") {
      for group in m.groups() {
        if let Some(states) = metadata.group_items::<State>("units", group.clone()) {
          for s in states {
            state_defs.push((Ustr::from(format!("{group}@{}", s.name)), s));
          }
        }
        if let Some(signals) = metadata.group_items::<Signal>("units", group.clone()) {
          for s in signals {
            signal_defs.push((Ustr::from(format!("{group}@{}", s.name)), s));
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
    mut payload: RuntimePayload,
    ctx: &mut RuntimeContext<'_>,
    dispatch: &RuntimeDispatcher,
    log: &LogHandle,
  ) -> Result<Option<RuntimePayload>, CoreError> {
    let (state_defs, signal_defs) = Self::collect_defs(ctx.registry.metadata);

    match action {
      "set_state" => {
        let name = payload.get::<Ustr>("name")?;
        let flow_payload = FlowPayload::from_json(payload.get::<serde_json::Value>("payload").ok());

        let sm = ctx
          .registry
          .singleton_mut::<StateMachine>(StateMachine::KEY)
          .ok_or(CoreError::InvalidState(
            "state machine store not found".into(),
          ))?;

        let mut guard = HashSet::new();
        Self::set_state(
          sm,
          name.clone(),
          Some(flow_payload.clone()),
          &state_defs,
          &signal_defs,
          &mut guard,
          ctx.event_bus,
          dispatch,
        )?;
        Self::save_state_machine(sm)?;

        let mut fields = HashMap::new();
        fields.insert("name".to_string(), name.to_string());
        fields.insert("payload".into(), flow_payload.to_string_payload());
        log.log(LogLevel::Trace, "flow-runtime", "setting state", fields);
      }
      "remove_state" => {
        let name = payload.get::<Ustr>("name")?;
        let filter_json: Option<serde_json::Value> = payload.get("filter").ok();
        let filter = filter_json.and_then(|v| serde_json::from_value(v).ok());

        let sm = ctx
          .registry
          .singleton_mut::<StateMachine>(StateMachine::KEY)
          .ok_or(CoreError::InvalidState(
            "state machine store not found".into(),
          ))?;
        let mut guard = HashSet::new();
        Self::remove_state(
          sm,
          name.as_str(),
          filter.clone(),
          &state_defs,
          &signal_defs,
          &mut guard,
          ctx.event_bus,
          dispatch,
        );
        Self::save_state_machine(sm)?;

        let mut fields = HashMap::new();
        fields.insert("name".to_string(), name.to_string());
        fields.insert("payload".into(), format!("{filter:?}"));
        log.log(LogLevel::Trace, "flow-runtime", "removing state", fields);
      }
      "emit_signal" => {
        let name = payload.get::<Ustr>("name")?;
        let flow_payload = FlowPayload::from_json(payload.get("payload").ok());
        Self::emit_signal(
          name.clone(),
          Some(flow_payload.clone()),
          &signal_defs,
          ctx.event_bus,
        )?;
        let source = FlowInstance {
          name,
          payload: flow_payload,
          r#type: FlowType::Signal,
        };
        let mut emitted = HashSet::new();
        let sm = ctx
          .registry
          .singleton_mut::<StateMachine>(StateMachine::KEY)
          .ok_or(CoreError::InvalidState(
            "state machine store not found".into(),
          ))?;
        Self::reconcile_signal_transcendence(
          sm,
          &source,
          &signal_defs,
          ctx.event_bus,
          &mut emitted,
        );
      }
      "bootstrap" => {
        Self::setup_all_state_subscribers(dispatch, &state_defs);

        let sm = ctx
          .registry
          .singleton_mut::<StateMachine>(StateMachine::KEY)
          .ok_or(CoreError::InvalidState(
            "state machine store not found".into(),
          ))?;
        let mut guard = HashSet::new();

        let existing_states = sm
          .states
          .values()
          .flat_map(|branches| branches.iter().cloned())
          .collect::<Vec<_>>();

        for state in &existing_states {
          Self::reconcile_transcendence(
            sm,
            state,
            FlowAction::Apply,
            &state_defs,
            &signal_defs,
            &mut guard,
            ctx.event_bus,
            dispatch,
          );
        }

        Self::reconcile_activate_on_none(
          sm,
          &state_defs,
          &signal_defs,
          &mut guard,
          ctx.event_bus,
          dispatch,
        );
        Self::save_state_machine(sm)?;
      }
      _ => {}
    }

    Ok(None)
  }
}

pub fn state_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_STATE_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/var/lib/system-state")
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

pub fn condition_matches(
  sm: &StateMachine,
  cond: &FlowItem,
  event: Option<&FlowInstance>,
  payload: Option<&FlowPayload>,
) -> bool {
  if let Some(event) = event {
    if check_condition(cond, event) && payload_compatible(payload, &event.payload) {
      return true;
    }
  }
  condition_is_active(sm, cond, payload)
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

#[derive(Default)]
pub struct FlowRuntimePayload<'a> {
  pub name: &'a str,
  pub payload: Option<serde_json::Value>,
  pub filter: Option<serde_json::Value>,
}

impl<'a> FlowRuntimePayload<'a> {
  pub fn new(name: &'a str) -> Self {
    Self {
      name,
      ..Default::default()
    }
  }

  pub fn payload(mut self, v: impl Into<serde_json::Value>) -> Self {
    self.payload = Some(v.into());
    self
  }

  pub fn filter(mut self, v: impl Into<serde_json::Value>) -> Self {
    self.filter = Some(v.into());
    self
  }
}

impl<'a> Into<RuntimePayload> for FlowRuntimePayload<'a> {
  fn into(self) -> RuntimePayload {
    let mut p = RuntimePayload::default().insert::<Ustr>("name", self.name.into());

    if let Some(p1) = self.payload {
      p = p.insert("payload", p1);
    }

    if let Some(p1) = self.filter {
      p = p.insert("filter", p1);
    }

    p
  }
}
