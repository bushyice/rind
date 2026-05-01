use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use rind_core::prelude::*;

use crate::transport::TransportMethod;
use crate::triggers::{
  branch_target_key, check_condition, default_payload_for_type, json_branch_key, map_json_payload,
  merge_json, payload_compatible, payload_signature, payload_to_filter,
};
use crate::variables::VariableHeap;

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
  pub variable: Option<String>,
  pub insert: Option<AutoPayloadInsert>,
  #[serde(default)]
  pub many: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum InverseBranchingConfig {
  Simple(Ustr),
  Detailed {
    name: Ustr,
    #[serde(alias = "branching")]
    branch: Option<Ustr>,
  },
}

impl InverseBranchingConfig {
  fn name(&self) -> &Ustr {
    match self {
      InverseBranchingConfig::Simple(name) => name,
      InverseBranchingConfig::Detailed { name, branch: _ } => name,
    }
  }

  fn branch(&self) -> Option<&Ustr> {
    match self {
      InverseBranchingConfig::Simple(_) => None,
      InverseBranchingConfig::Detailed { name: _, branch } => branch.as_ref(),
    }
  }
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
  pub activate_on_none: Option<Vec<InverseBranchingConfig>>,
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

pub struct FlowRuntime {
  state_defs: Vec<(Ustr, Arc<StateMetadata>)>,
  signal_defs: Vec<(Ustr, Arc<SignalMetadata>)>,
  activate_on_none_index: HashMap<Ustr, HashSet<Ustr>>,
  transcendence_index: HashMap<Ustr, HashSet<Ustr>>,
}

impl Default for FlowRuntime {
  fn default() -> Self {
    Self {
      state_defs: Vec::new(),
      signal_defs: Vec::new(),
      activate_on_none_index: HashMap::new(),
      transcendence_index: HashMap::new(),
    }
  }
}

impl FlowRuntime {
  fn transport_id<'a>(&self, subscriber: &'a TransportMethod) -> &'a str {
    match subscriber {
      TransportMethod::Type(id) => id.0.as_str(),
      TransportMethod::Options { id, .. } => id.0.as_str(),
      TransportMethod::Object { id, .. } => id.0.as_str(),
    }
  }

  fn setup_subscriber_endpoint(
    &self,
    dispatch: &RuntimeDispatcher,
    endpoint: &str,
    subscriber: &TransportMethod,
  ) {
    if self.transport_id(subscriber) == "uds" {
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
    &self,
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
      self.setup_subscriber_endpoint(dispatch, endpoint, subscriber);
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
    &self,
    dispatch: &RuntimeDispatcher,
    state_defs: &[(Ustr, Arc<StateMetadata>)],
  ) {
    for (name, def) in state_defs {
      if let Some(subscribers) = def.subscribers.as_deref() {
        for subscriber in subscribers {
          self.setup_subscriber_endpoint(dispatch, name, subscriber);
        }
      }
    }
  }

  fn state_subscribers_for<'a>(
    &self,
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

  fn save_state_machine(&self, sm: &StateMachine) -> Result<(), CoreError> {
    let snapshot = sm.snapshot_for_persistence();
    sm.persistence.save(snapshot);
    Ok(())
  }

  fn payload_type_ok(&self, expected: FlowPayloadType, payload: &FlowPayload) -> bool {
    match payload {
      FlowPayload::Json(_) => expected == FlowPayloadType::Json,
      FlowPayload::String(_) => expected == FlowPayloadType::String,
      FlowPayload::Bytes(_) => expected == FlowPayloadType::Bytes,
      FlowPayload::None(_) => expected == FlowPayloadType::None,
    }
  }

  fn set_state(
    &mut self,
    sm: &mut StateMachine,
    name: impl Into<Ustr>,
    payload: Option<FlowPayload>,
    variables: Option<&VariableHeap>,
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

    let def = self
      .state_defs
      .iter()
      .find(|(full_name, _)| *full_name == name)
      .or_else(|| {
        let item_name = name
          .as_str()
          .split_once('@')
          .map(|(_, n)| n)
          .unwrap_or(name.as_str());
        self
          .state_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
      })
      .map(|(_, d)| d.clone())
      .ok_or_else(|| CoreError::InvalidState(format!("state not found: {name}")))?;

    let flow_payload = payload.unwrap_or(FlowPayload::None(false));
    if !self.payload_type_ok(def.payload, &flow_payload) {
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
    self.publish_to_state_subscribers(
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

    self.reconcile_transcendence(
      sm,
      &instance,
      FlowAction::Apply,
      variables,
      guard,
      event_bus,
      dispatch,
    );
    self.reconcile_activate_on_none_for_source(
      sm,
      &instance,
      FlowAction::Apply,
      variables,
      guard,
      event_bus,
      dispatch,
    )?;

    guard.remove(&guard_key);
    Ok(())
  }

  fn remove_state(
    &mut self,
    sm: &mut StateMachine,
    name: &str,
    filter: Option<FlowMatchOperation>,
    variables: Option<&VariableHeap>,
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
        let subscribers = self.state_subscribers_for(name, &self.state_defs);
        self.publish_to_state_subscribers(
          dispatch,
          branch.name.as_str(),
          &branch.payload,
          FlowAction::Revert,
          subscribers,
        );

        self.reconcile_transcendence(
          sm,
          &branch,
          FlowAction::Revert,
          variables,
          guard,
          event_bus,
          dispatch,
        );
        let _ = self.reconcile_activate_on_none_for_source(
          sm,
          &branch,
          FlowAction::Revert,
          variables,
          guard,
          event_bus,
          dispatch,
        );
        guard.remove(&guard_key);
      }

      if !to_keep.is_empty() {
        sm.states.insert(Ustr::from(name.to_string()), to_keep);
      }
    }
  }

  fn emit_signal(
    &self,
    name: impl Into<Ustr>,
    payload: Option<FlowPayload>,
    event_bus: &EventBus,
  ) -> Result<(), CoreError> {
    let name = name.into();
    let def = self
      .signal_defs
      .iter()
      .find(|(full_name, _)| *full_name == name)
      .or_else(|| {
        let item_name = name
          .as_str()
          .split_once('@')
          .map(|(_, n)| n)
          .unwrap_or(name.as_str());
        self
          .signal_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
      })
      .map(|(_, d)| d.clone())
      .ok_or_else(|| CoreError::InvalidState(format!("signal not found: {name}")))?;

    let flow_payload = payload.unwrap_or(FlowPayload::None(false));
    if !self.payload_type_ok(def.payload, &flow_payload) {
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
    &self,
    sm: &StateMachine,
    source: &FlowInstance,
    event_bus: &EventBus,
    emitted: &mut HashSet<Ustr>,
  ) {
    let dependents: Vec<(Ustr, FlowPayload)> = self
      .signal_defs
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
      let _ = self.emit_signal(signal_name, Some(payload), event_bus);
    }
  }

  fn reconcile_transcendence(
    &mut self,
    sm: &mut StateMachine,
    source: &FlowInstance,
    action: FlowAction,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) {
    let Some(targets) = self.transcendence_index.get(&source.name) else {
      return;
    };

    let dependents: Vec<(Ustr, FlowPayload)> = self
      .state_defs
      .iter()
      .filter(|(full_name, _)| targets.contains(full_name))
      .filter_map(|(full_name, def)| {
        let after = def.after.as_ref()?;
        if !after.iter().any(|cond| check_condition(cond, source)) {
          return None;
        }

        let source_payload = if def.auto_payload.is_some() {
          let payloads = auto_payloads_for(def, Some(&source.payload), variables);
          let Some(first) = payloads.first().cloned() else {
            return None;
          };
          first
        } else {
          source.payload.clone()
        };

        let payload = transcendent_payload_for(def, &source_payload)?;

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
          let _ = self.set_state(
            sm,
            dependent,
            Some(payload),
            variables,
            guard,
            event_bus,
            dispatch,
          );
        }
        FlowAction::Revert => {
          self.remove_state(
            sm,
            &dependent,
            payload_to_filter(&payload),
            variables,
            guard,
            event_bus,
            dispatch,
          );
        }
      }
    }
  }

  fn branch_filter_from_payload(
    &self,
    payload: &FlowPayload,
    branch_spec: &str,
  ) -> Option<FlowMatchOperation> {
    let FlowPayload::Json(json) = payload else {
      return None;
    };
    let data = json.into_json();
    let mut current = &data;
    for segment in branch_spec.split('/').filter(|p| !p.is_empty()) {
      current = current.get(segment)?;
    }
    let mut obj = serde_json::Map::new();
    obj.insert(branch_target_key(branch_spec).to_string(), current.clone());
    Some(FlowMatchOperation::Options {
      binary: None,
      contains: None,
      r#as: Some(serde_json::Value::Object(obj)),
    })
  }

  fn reconcile_activate_on_none_for_source(
    &mut self,
    sm: &mut StateMachine,
    source: &FlowInstance,
    action: FlowAction,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<(), CoreError> {
    let Some(targets) = self.activate_on_none_index.get(&source.name) else {
      return Ok(());
    };
    let target_names: Vec<Ustr> = targets.iter().cloned().collect();

    for full_name in target_names {
      let Some(def) = self
        .state_defs
        .iter()
        .find(|(name, _)| *name == full_name)
        .map(|(_, d)| d.clone())
      else {
        continue;
      };
      let Some(deps): Option<Vec<InverseBranchingConfig>> = def.activate_on_none.clone() else {
        continue;
      };

      for payload in auto_payloads_for(&def, None, variables) {
        let should_activate = deps.iter().all(|cfg| {
          let branches = sm
            .states
            .get(cfg.name())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
          if let Some(branch_spec) = cfg.branch() {
            let filter = self.branch_filter_from_payload(&payload, branch_spec.as_str());
            !branches.iter().any(|b| {
              if let Some(filter) = &filter {
                crate::triggers::match_operation(filter, &b.payload)
              } else {
                false
              }
            })
          } else {
            if matches!(action, FlowAction::Apply) && cfg.name() == &source.name {
              return branches.is_empty();
            }
            branches.is_empty()
          }
        });

        let currently_active = sm
          .states
          .get(&full_name)
          .map(|branches| {
            branches
              .iter()
              .any(|b| payload_compatible(Some(&payload), &b.payload))
          })
          .unwrap_or(false);

        if should_activate && !currently_active {
          self.set_state(
            sm,
            full_name.clone(),
            Some(payload.clone()),
            variables,
            guard,
            event_bus,
            dispatch,
          )?;
        } else if !should_activate && currently_active {
          self.remove_state(
            sm,
            full_name.as_str(),
            payload_to_filter(&payload),
            variables,
            guard,
            event_bus,
            dispatch,
          );
        }
      }
    }

    Ok(())
  }

  fn reconcile_activate_on_none_all(
    &mut self,
    sm: &mut StateMachine,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<(), CoreError> {
    let sources: Vec<Ustr> = self.activate_on_none_index.keys().cloned().collect();
    for source in sources {
      let source_instance = FlowInstance {
        name: source,
        payload: FlowPayload::None(false),
        r#type: FlowType::State,
      };
      self.reconcile_activate_on_none_for_source(
        sm,
        &source_instance,
        FlowAction::Revert,
        variables,
        guard,
        event_bus,
        dispatch,
      )?;
    }

    Ok(())
  }

  fn collect_defs(
    &self,
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

  fn refresh_metadata_and_indexes(&mut self, metadata: &MetadataRegistry) {
    let (state_defs, signal_defs) = self.collect_defs(metadata);
    self.state_defs = state_defs;
    self.signal_defs = signal_defs;
    self.rebuild_activate_on_none_index();
    self.rebuild_transcendence_index();
  }

  fn rebuild_activate_on_none_index(&mut self) {
    self.activate_on_none_index.clear();
    for (full_name, def) in &self.state_defs {
      let Some(deps) = &def.activate_on_none else {
        continue;
      };
      for dep in deps {
        self
          .activate_on_none_index
          .entry(dep.name().clone())
          .or_default()
          .insert(full_name.clone());
      }
    }
  }

  fn rebuild_transcendence_index(&mut self) {
    self.transcendence_index.clear();
    for (full_name, def) in &self.state_defs {
      let Some(after) = &def.after else {
        continue;
      };
      for cond in after {
        if let Some(name) = self.condition_name(cond) {
          self
            .transcendence_index
            .entry(name)
            .or_default()
            .insert(full_name.clone());
        }
      }
    }
  }

  fn condition_name(&self, cond: &FlowItem) -> Option<Ustr> {
    match cond {
      FlowItem::Simple(name) => Some(name.clone()),
      FlowItem::Detailed {
        state,
        signal: _,
        target: _,
        branch: _,
      } => state.clone(),
    }
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
    self.refresh_metadata_and_indexes(ctx.registry.metadata);

    match action {
      "set_state" => {
        let name = payload.get::<Ustr>("name")?;
        let flow_payload = FlowPayload::from_json(payload.get::<serde_json::Value>("payload").ok());
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |_, (sm, vh)| {
              let mut guard = HashSet::new();
              self.set_state(
                sm,
                name.clone(),
                Some(flow_payload.clone()),
                Some(vh),
                &mut guard,
                ctx.event_bus,
                dispatch,
              )?;
              self.save_state_machine(sm)
            },
          )?;

        let mut fields = HashMap::new();
        fields.insert("name".to_string(), name.to_string());
        fields.insert("payload".into(), flow_payload.to_string_payload());
        log.log(LogLevel::Trace, "flow-runtime", "setting state", fields);
      }
      "remove_state" => {
        let name = payload.get::<Ustr>("name")?;
        let filter_json: Option<serde_json::Value> = payload.get("filter").ok();
        let filter = filter_json.and_then(|v| serde_json::from_value(v).ok());

        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |_, (sm, vh)| {
              let mut guard = HashSet::new();
              self.remove_state(
                sm,
                name.as_str(),
                filter.clone(),
                Some(vh),
                &mut guard,
                ctx.event_bus,
                dispatch,
              );
              self.save_state_machine(sm)
            },
          )?;

        let mut fields = HashMap::new();
        fields.insert("name".to_string(), name.to_string());
        fields.insert("payload".into(), format!("{filter:?}"));
        log.log(LogLevel::Trace, "flow-runtime", "removing state", fields);
      }
      "emit_signal" => {
        let name = payload.get::<Ustr>("name")?;
        let flow_payload = FlowPayload::from_json(payload.get("payload").ok());
        self.emit_signal(
          name.clone(),
          Some(flow_payload.clone()),
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
        self.reconcile_signal_transcendence(
          sm,
          &source,
          ctx.event_bus,
          &mut emitted,
        );
      }
      "bootstrap" => {
        self.setup_all_state_subscribers(dispatch, &self.state_defs);
        ctx
          .registry
          .singleton_handle::<(&mut StateMachine, &mut VariableHeap), _>(
            (StateMachine::KEY.into(), VariableHeap::KEY.into()),
            |_, (sm, vh)| {
              let mut guard = HashSet::new();

              let existing_states = sm
                .states
                .values()
                .flat_map(|branches| branches.iter().cloned())
                .collect::<Vec<_>>();

              for state in &existing_states {
                self.reconcile_transcendence(
                  sm,
                  state,
                  FlowAction::Apply,
                  Some(vh),
                  &mut guard,
                  ctx.event_bus,
                  dispatch,
                );
              }

              self.reconcile_activate_on_none_all(
                sm,
                Some(vh),
                &mut guard,
                ctx.event_bus,
                dispatch,
              )?;
              self.save_state_machine(sm)
            },
          )?;
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

fn payloads_from_toml(
  def: &StateMetadata,
  cfg: &AutoPayloadConfig,
  toml_value: toml::Value,
) -> FlowPayload {
  match def.payload {
    FlowPayloadType::Json => match &cfg.insert {
      Some(AutoPayloadInsert::One(key)) if key == "root" => FlowPayload::Json(
        serde_json::to_value(toml_value)
          .unwrap_or(serde_json::Value::Bool(true))
          .to_string()
          .into(),
      ),
      Some(AutoPayloadInsert::One(key)) => FlowPayload::Json(
        serde_json::json!({ key: serde_json::to_value(toml_value).unwrap_or(serde_json::Value::Bool(true)) })
        .to_string()
        .into(),
      ),
      Some(AutoPayloadInsert::Many(keys)) => {
        let mut obj = serde_json::Map::new();
        for (i, key) in keys.iter().enumerate() {
          if let Some(value) = &toml_value.get(i) {
            obj.insert(key.clone(), serde_json::to_value(value).unwrap_or(serde_json::Value::Bool(true)));
          }
        }
        FlowPayload::Json(
          serde_json::Value::Object(obj).to_string().into(),
        )
      }
      None => FlowPayload::Json(serde_json::json!({ "value": "none" }).to_string().into()),
    },
    FlowPayloadType::String => FlowPayload::String(toml_value.to_string()),
    FlowPayloadType::Bytes => FlowPayload::Bytes(toml_value.to_string().as_bytes().to_vec()),
    FlowPayloadType::None => FlowPayload::None(false),
  }
}

fn auto_payloads_for(
  def: &StateMetadata,
  _payload: Option<&FlowPayload>,
  variables: Option<&VariableHeap>,
) -> Vec<FlowPayload> {
  let Some(cfg) = &def.auto_payload else {
    return vec![default_payload_for_type(def.payload)];
  };
  let Some(variable) = &cfg.variable else {
    return vec![default_payload_for_type(def.payload)];
  };

  let Some(value) = variables.and_then(|v| v.get(variable)) else {
    return vec![default_payload_for_type(def.payload)];
  };

  if cfg.many {
    let Some(value) = value.as_array() else {
      return vec![default_payload_for_type(def.payload)];
    };
    value
      .iter()
      .map(|x| payloads_from_toml(def, cfg, x.clone()))
      .collect()
  } else {
    vec![payloads_from_toml(def, cfg, value)]
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
