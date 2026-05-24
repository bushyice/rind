pub mod shm_tp;
pub mod transport;
pub mod triggers;

use rind_primitives::scopes::GLOBAL_SCOPE_STORE;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use rind_core::prelude::*;
use rind_core::reexports::*;
pub use rind_ipc::{FlowJson, FlowMatchOperation, FlowPayload, FlowPayloadType};

use crate::transport::TransportMethod;
use crate::triggers::{
  branch_target_key, check_condition, default_payload_for_type, json_branch_key, map_json_payload,
  merge_json, payload_compatible, payload_signature, payload_to_filter,
};
use rind_primitives::prelude::ScopeStore;
use rind_primitives::prelude::VariableHeap;

pub const FLOW_RUNTIME_ID: &str = "flow";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum FlowItem {
  Simple(Ustr),
  Detailed {
    #[serde(alias = "state")]
    facet: Option<Ustr>,
    #[serde(alias = "signal")]
    impulse: Option<Ustr>,
    target: Option<FlowMatchOperation>,
    branch: Option<FlowMatchOperation>,
  },
}

impl FlowItem {
  pub fn name(&self) -> &Ustr {
    match self {
      FlowItem::Simple(s) => s,
      FlowItem::Detailed {
        facet,
        impulse,
        target: _,
        branch: _,
      } => {
        if let Some(facet) = facet {
          facet
        } else {
          impulse.as_ref().unwrap()
        }
      }
    }
  }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Trigger {
  pub script: Option<Ustr>,
  pub exec: Option<Ustr>,
  pub args: Option<Vec<Ustr>>,
  pub facet: Option<Ustr>,
  pub impulse: Option<Ustr>,
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
  Impulse,
  Facet,
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
      r#type: FlowType::Facet,
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
    name, payload, stop_on, after, branch, auto_payload, subscribers, broadcast, permissions
  ),
  derive_metadata(Debug, Clone)
)]
pub struct FlowFacet {
  pub name: Ustr,
  pub payload: FlowPayloadType,
  #[serde(rename = "stop-on")]
  pub stop_on: Option<Vec<InverseBranchingConfig>>,
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
pub struct FlowImpulse {
  pub name: Ustr,
  pub payload: FlowPayloadType,
  pub after: Option<Vec<FlowItem>>,
  pub branch: Option<Vec<Ustr>>,
  pub subscribers: Option<Vec<TransportMethod>>,
  pub broadcast: Option<Vec<Ustr>>,
  pub permissions: Option<Vec<Ustr>>,
}

#[derive(Clone)]
pub struct FacetGraph {
  pub facets: HashMap<Ustr, Vec<FlowInstance>>,
  persistence: StatePersistence,
  persistence_root: PathBuf,
  scoped_persistence: HashMap<Ustr, StatePersistence>,
}

impl FacetGraph {
  pub const KEY: &str = "runtime:facet_graph";

  pub fn from_persistence(persistence: StatePersistence) -> Self {
    Self {
      persistence: persistence,
      persistence_root: state_root_path(),
      scoped_persistence: HashMap::new(),
      facets: Default::default(),
    }
  }

  pub fn load_from_persistence(&mut self) -> Result<Void, CoreError> {
    self.facets = self
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

    if self.persistence_root.exists() {
      if let Ok(entries) = std::fs::read_dir(&self.persistence_root) {
        for entry in entries.flatten() {
          if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
          }
          let scope = entry
            .file_name()
            .to_str()
            .map(Ustr::from)
            .unwrap_or_else(|| Ustr::from("static"));
          if scope == Ustr::from("static") {
            continue;
          }
          let _ = self.load_scope_from_persistence(scope.as_str());
        }
      }
    }
    Ok(Void)
  }

  fn scope_from_state_name(name: &str) -> Ustr {
    let mut parts = name.rsplitn(2, '@');
    let scope = parts.next().unwrap_or("static");
    let left = parts.next();
    if left.is_some() {
      Ustr::from(scope)
    } else {
      Ustr::from("static")
    }
  }

  fn scoped_state_path(root: &PathBuf, scope: &str) -> PathBuf {
    root.join(scope).join("state.bin")
  }

  fn persistence_for_scope(&mut self, scope: &str) -> StatePersistence {
    if scope == "static" {
      return self.persistence.clone();
    }
    if let Some(p) = self.scoped_persistence.get(&Ustr::from(scope)) {
      return p.clone();
    }
    let p = StatePersistence::new(Self::scoped_state_path(&self.persistence_root, scope));
    self.scoped_persistence.insert(Ustr::from(scope), p.clone());
    p
  }

  pub fn load_scope_from_persistence(&mut self, scope: &str) -> Result<Void, CoreError> {
    let persistence = self.persistence_for_scope(scope);
    let snapshot = persistence.load()?;
    for (name, entries) in snapshot {
      let key = Ustr::from(name);
      let vals = entries
        .into_iter()
        .map(FlowInstance::from)
        .filter(|x| !x.name.as_str().is_empty())
        .collect::<Vec<_>>();
      self.facets.insert(key, vals);
    }
    Ok(Void)
  }

  pub fn drop_scope(&mut self, scope: &str) -> Result<Void, CoreError> {
    let suffix = format!("@{scope}");
    self.facets.retain(|k, _| {
      scope == "static" && !k.as_str().contains('@') || !k.as_str().ends_with(&suffix)
    });

    if scope != "static" {
      let scope_dir = self.persistence_root.join(scope);
      if scope_dir.exists() {
        let _ = std::fs::remove_dir_all(scope_dir);
      }
      self.scoped_persistence.remove(&Ustr::from(scope));
    }

    Ok(Void)
  }

  pub fn save_all_scopes(&mut self) -> Result<Void, CoreError> {
    let mut per_scope: HashMap<Ustr, StateSnapshot> = HashMap::new();
    for (name, branches) in &self.facets {
      if name.as_str().contains(":_") {
        continue;
      }
      let scope = Self::scope_from_state_name(name.as_str());
      per_scope.entry(scope).or_default().insert(
        name.to_string(),
        branches.iter().map(StateEntry::from).collect::<Vec<_>>(),
      );
    }
    per_scope.entry(Ustr::from("static")).or_default();

    for (scope, snapshot) in per_scope {
      let persistence = self.persistence_for_scope(scope.as_str());
      persistence.save_sync(&snapshot)?;
    }

    Ok(Void)
  }

  pub fn snapshot_for_persistence(&self) -> StateSnapshot {
    self
      .facets
      .iter()
      .filter_map(|(name, states)| {
        // State impermanence
        if name.as_str().contains(":_") {
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
  facet_defs: HashMap<Ustr, Arc<FlowFacetMetadata>>,
  impulse_defs: HashMap<Ustr, Arc<FlowImpulseMetadata>>,
  inverse_transcendence_index: HashMap<Ustr, HashSet<Ustr>>,
  transcendence_index: HashMap<Ustr, HashSet<Ustr>>,
}

impl Default for FlowRuntime {
  fn default() -> Self {
    Self {
      facet_defs: HashMap::new(),
      impulse_defs: HashMap::new(),
      inverse_transcendence_index: HashMap::new(),
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
    let id = self.transport_id(subscriber);
    if id == "uds" || id == "shm" {
      if id == "uds" {
        let mut act = crate::transport::TransportRuntime::actions.setup_uds(endpoint.to_ustr());
        if let Some(perms) = subscriber.get_permissions() {
          act = act.permissions(perms);
        }
        let _ = act.dispatch(dispatch);
      } else {
        let mut act = crate::transport::TransportRuntime::actions.setup_shm(endpoint.to_ustr());
        if let Some(perms) = subscriber.get_permissions() {
          act = act.permissions(perms);
        }
        let _ = act.dispatch(dispatch);
      }
    }
  }

  fn publish_to_facet_subscribers(
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
      let _ = crate::transport::TransportRuntime::actions
        .send(endpoint.to_ustr())
        .r#type("facet".to_string())
        .name(endpoint.to_ustr())
        .action(action.to_string())
        .payload(payload.to_json())
        .dispatch(dispatch);
    }
  }

  fn setup_all_facet_subscribers(&self, dispatch: &RuntimeDispatcher) {
    for (name, def) in &self.facet_defs {
      if let Some(subscribers) = def.subscribers.as_deref() {
        for subscriber in subscribers {
          self.setup_subscriber_endpoint(dispatch, name.as_str(), subscriber);
        }
      }
    }
  }

  fn facet_subscribers_for(&self, name: &Ustr) -> Option<Vec<TransportMethod>> {
    self
      .facet_defs
      .get(name)
      .and_then(|d| d.subscribers.clone())
  }

  fn save_facet_graph(&self, sm: &mut FacetGraph) -> Result<Void, CoreError> {
    sm.save_all_scopes()?;
    Ok(Void)
  }

  fn payload_type_ok(&self, expected: FlowPayloadType, payload: &FlowPayload) -> bool {
    match payload {
      FlowPayload::Json(_) => expected == FlowPayloadType::Json,
      FlowPayload::String(_) => expected == FlowPayloadType::String,
      FlowPayload::Bytes(_) => expected == FlowPayloadType::Bytes,
      FlowPayload::None(_) => expected == FlowPayloadType::None,
    }
  }

  fn set_facet(
    &mut self,
    sm: &mut FacetGraph,
    name: impl Into<Ustr>,
    payload: Option<FlowPayload>,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<Void, CoreError> {
    let name = name.into();
    let branch_sig = payload_signature(&payload);
    let guard_key = Ustr::from(format!("apply::{name}::{branch_sig}"));
    if guard.contains(&guard_key) {
      return Ok(Void);
    }
    guard.insert(guard_key.clone());

    let def = self
      .facet_defs
      .get(&name)
      .or_else(|| {
        let item_name = name
          .as_str()
          .split_once(':')
          .map(|(_, n)| n)
          .unwrap_or(name.as_str());
        self
          .facet_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
          .map(|(_, d)| d)
      })
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
      r#type: FlowType::Facet,
    };

    event_bus.emit(FlowEvent {
      name: name.clone(),
      payload: instance.payload.to_json(),
      action: FlowAction::Apply,
      flow_type: FlowEventType::Facet,
    });
    self.publish_to_facet_subscribers(
      dispatch,
      name.as_str(),
      &instance.payload,
      FlowAction::Apply,
      def.subscribers.as_deref(),
    );

    let entry = sm.facets.entry(name).or_default();
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
    )?;
    self.reconcile_inverse_transcendence_for_source(
      sm,
      &instance,
      FlowAction::Apply,
      variables,
      guard,
      event_bus,
      dispatch,
    )?;

    guard.remove(&guard_key);
    Ok(Void)
  }

  fn remove_facet(
    &mut self,
    sm: &mut FacetGraph,
    name: &str,
    filter: Option<FlowMatchOperation>,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> CoreResult<Void> {
    if let Some(branches) = sm.facets.remove(name) {
      let (to_keep, to_remove): (Vec<_>, Vec<_>) = if let Some(filter) = &filter {
        branches
          .into_iter()
          .partition(|b| !crate::triggers::match_operation(filter, &b.payload))
      } else {
        (Vec::new(), branches)
      };

      for mut branch in to_remove {
        branch.r#type = FlowType::Facet;
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
          flow_type: FlowEventType::Facet,
        });
        let subscribers = self.facet_subscribers_for(&name.to_ustr());
        self.publish_to_facet_subscribers(
          dispatch,
          branch.name.as_str(),
          &branch.payload,
          FlowAction::Revert,
          subscribers.as_deref(),
        );

        self.reconcile_transcendence(
          sm,
          &branch,
          FlowAction::Revert,
          variables,
          guard,
          event_bus,
          dispatch,
        )?;
        let _ = self.reconcile_inverse_transcendence_for_source(
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
        sm.facets.insert(Ustr::from(name.to_string()), to_keep);
      }
    }

    Ok(Void)
  }

  fn impulse(
    &self,
    name: impl Into<Ustr>,
    payload: Option<FlowPayload>,
    event_bus: &EventBus,
  ) -> Result<Void, CoreError> {
    let name = name.into();
    let def = self
      .impulse_defs
      .get(&name)
      .or_else(|| {
        let item_name = name
          .as_str()
          .split_once(':')
          .map(|(_, n)| n)
          .unwrap_or(name.as_str());
        self
          .impulse_defs
          .iter()
          .find(|(_, d)| d.name.as_str() == item_name)
          .map(|(_, d)| d)
      })
      .map(|d| d.clone())
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
      flow_type: FlowEventType::Impulse,
    });

    Ok(Void)
  }

  fn reconcile_impulse_transcendence(
    &self,
    sm: &FacetGraph,
    source: &FlowInstance,
    event_bus: &EventBus,
    emitted: &mut HashSet<Ustr>,
  ) {
    let dependents: Vec<(Ustr, FlowPayload)> = self
      .impulse_defs
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
      let _ = self.impulse(signal_name, Some(payload), event_bus);
    }
  }

  fn reconcile_transcendence(
    &mut self,
    sm: &mut FacetGraph,
    source: &FlowInstance,
    action: FlowAction,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> CoreResult<Void> {
    let Some(targets) = self.transcendence_index.get(&source.name).cloned() else {
      return Ok(Void);
    };

    for full_name in targets {
      let Some(def) = self.facet_defs.get(&full_name) else {
        continue;
      };

      let Some(after) = def.after.as_ref() else {
        continue;
      };
      if !after.iter().any(|cond| check_condition(cond, source)) {
        continue;
      }

      let source_payload = if def.auto_payload.is_some() {
        let payloads = auto_payloads_for(def, Some(&source.payload), variables);
        let Some(first) = payloads.first().cloned() else {
          continue;
        };
        first
      } else {
        source.payload.clone()
      };

      let Some(payload) = transcendent_payload_for(def, &source_payload) else {
        continue;
      };

      // let all_active = after
      //   .iter()
      //   .all(|cond| condition_matches(sm, cond, Some(source), Some(&payload)));

      match action {
        FlowAction::Apply => self.set_facet(
          sm,
          full_name,
          Some(payload),
          variables,
          guard,
          event_bus,
          dispatch,
        )?,
        FlowAction::Revert => self.remove_facet(
          sm,
          &full_name,
          payload_to_filter(&payload),
          variables,
          guard,
          event_bus,
          dispatch,
        )?,
        // _ => {}
      }
    }

    Ok(Void)
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

  fn reconcile_inverse_transcendence_for_source(
    &mut self,
    sm: &mut FacetGraph,
    source: &FlowInstance,
    action: FlowAction,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<Void, CoreError> {
    let Some(targets) = self.inverse_transcendence_index.get(&source.name) else {
      return Ok(Void);
    };
    let target_names: Vec<Ustr> = targets.iter().cloned().collect();

    for full_name in target_names {
      let Some(def) = self.facet_defs.get(&full_name).cloned() else {
        continue;
      };
      let Some(deps): Option<Vec<InverseBranchingConfig>> = def.stop_on.clone() else {
        continue;
      };

      let auto_activate = def.auto_payload.is_some();

      for payload in if auto_activate {
        auto_payloads_for(&def, None, variables)
      } else {
        sm.facets.get(&full_name).map_or(Vec::new(), |x| {
          x.iter().map(|x| x.payload.clone()).collect()
        })
      } {
        let should_activate = auto_activate
          && deps.iter().all(|cfg| {
            let branches = sm
              .facets
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
              if matches!(action, FlowAction::Revert) && cfg.name() == &source.name {
                return true;
              }
              branches.is_empty()
            }
          });

        let currently_active = sm
          .facets
          .get(&full_name)
          .map(|branches| {
            branches
              .iter()
              .any(|b| payload_compatible(Some(&payload), &b.payload))
          })
          .unwrap_or(false);

        if should_activate && !currently_active {
          self.set_facet(
            sm,
            full_name.clone(),
            Some(payload.clone()),
            variables,
            guard,
            event_bus,
            dispatch,
          )?;
        } else if !should_activate && currently_active {
          self.remove_facet(
            sm,
            full_name.as_str(),
            payload_to_filter(&payload),
            variables,
            guard,
            event_bus,
            dispatch,
          )?;
        }
      }
    }

    Ok(Void)
  }

  fn reconcile_inverse_transcendence_all(
    &mut self,
    sm: &mut FacetGraph,
    variables: Option<&VariableHeap>,
    guard: &mut HashSet<Ustr>,
    event_bus: &EventBus,
    dispatch: &RuntimeDispatcher,
  ) -> Result<Void, CoreError> {
    let sources: Vec<Ustr> = self.inverse_transcendence_index.keys().cloned().collect();
    for source in sources {
      let source_instance = FlowInstance {
        name: source,
        payload: FlowPayload::None(false),
        r#type: FlowType::Facet,
      };
      self.reconcile_inverse_transcendence_for_source(
        sm,
        &source_instance,
        FlowAction::Revert,
        variables,
        guard,
        event_bus,
        dispatch,
      )?;
    }

    Ok(Void)
  }

  fn collect_defs(
    &self,
    metadata: &MetadataRegistry,
  ) -> (
    HashMap<Ustr, Arc<FlowFacetMetadata>>,
    HashMap<Ustr, Arc<FlowImpulseMetadata>>,
  ) {
    let mut state_defs = HashMap::new();
    let mut signal_defs = HashMap::new();

    for meta_name in metadata.metadata_names() {
      let Some(m) = metadata.metadata(meta_name.clone()) else {
        continue;
      };
      for group in m.groups() {
        if let Some(states) = metadata.group_items::<FlowFacet>(meta_name.clone(), group.clone()) {
          for s in states {
            let key = Ustr::from(format!("{group}:{}@{}", s.name, meta_name));
            state_defs.insert(key.clone(), s.clone());
            state_defs.insert(Ustr::from(format!("{group}:{}", s.name)), s);
          }
        }
        if let Some(signals) = metadata.group_items::<FlowImpulse>(meta_name.clone(), group.clone())
        {
          for s in signals {
            let key = Ustr::from(format!("{group}:{}@{}", s.name, meta_name));
            signal_defs.insert(key.clone(), s.clone());
            signal_defs.insert(Ustr::from(format!("{group}:{}", s.name)), s);
          }
        }
      }
    }

    (state_defs, signal_defs)
  }

  fn refresh_metadata_and_indexes(&mut self, metadata: &MetadataRegistry) {
    let (state_defs, signal_defs) = self.collect_defs(metadata);
    self.facet_defs = state_defs;
    self.impulse_defs = signal_defs;
    self.rebuild_inverse_transcendence_index();
    self.rebuild_transcendence_index();
  }

  fn rebuild_inverse_transcendence_index(&mut self) {
    self.inverse_transcendence_index.clear();
    for (full_name, def) in &self.facet_defs {
      let Some(deps) = &def.stop_on else {
        continue;
      };
      for dep in deps {
        self
          .inverse_transcendence_index
          .entry(dep.name().clone())
          .or_default()
          .insert(full_name.clone());
      }
    }
  }

  fn rebuild_transcendence_index(&mut self) {
    self.transcendence_index.clear();
    for (full_name, def) in &self.facet_defs {
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
        facet: state,
        impulse: _,
        target: _,
        branch: _,
      } => state.clone(),
    }
  }
}

#[runtime("flow")]
impl FlowRuntime {
  fn set_facet(&mut self, name: Ustr, #[optional] payload: serde_json::Value) {
    let flow_payload = FlowPayload::from_json(payload);
    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), _>(
        (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
        |_, (sm, vh)| {
          let mut guard = HashSet::new();
          self.set_facet(
            sm,
            name.clone(),
            Some(flow_payload.clone()),
            Some(vh),
            &mut guard,
            ctx.event_bus,
            dispatch,
          )?;
          self.save_facet_graph(sm)
        },
      )?;

    let mut fields = HashMap::new();
    fields.insert("name".to_string(), name.to_string());
    fields.insert("payload".into(), flow_payload.to_string_payload());
    log.log(LogLevel::Trace, "flow-runtime", "setting state", fields);

    if let Some(notifier) = &ctx.notifier {
      notifier.notify()?;
    }
  }

  fn remove_facet(
    &mut self,
    name: Ustr,
    #[optional] filter: serde_json::Value,
    #[optional] payload: serde_json::Value,
  ) {
    let filter_json = filter.or(payload);
    let filter = filter_json.and_then(|v| match v {
      serde_json::Value::Object(b) => Some(FlowMatchOperation::Options {
        binary: None,
        contains: None,
        r#as: Some(b.into()),
      }),
      _ => serde_json::from_value(v).ok(),
    });

    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), _>(
        (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
        |_, (sm, vh)| {
          let mut guard = HashSet::new();
          self.remove_facet(
            sm,
            name.as_str(),
            filter.clone(),
            Some(vh),
            &mut guard,
            ctx.event_bus,
            dispatch,
          )?;
          self.save_facet_graph(sm)
        },
      )?;

    let mut fields = HashMap::new();
    fields.insert("name".to_string(), name.to_string());
    fields.insert("payload".into(), format!("{filter:?}"));
    log.log(LogLevel::Trace, "flow-runtime", "removing state", fields);

    if let Some(notifier) = &ctx.notifier {
      notifier.notify()?;
    }

    if let Some(sm) = ctx.registry.singleton::<FacetGraph>(FacetGraph::KEY) {
      let should_drop_scope = sm
        .facets
        .get(&name)
        .map(|branches| branches.is_empty())
        .unwrap_or(true);
      if should_drop_scope {
        let scope_name = GLOBAL_SCOPE_STORE
          .lock()
          .ok()
          .and_then(|s| s.scope_for_state(name.as_str()));
        if let Some(scope_name) = scope_name {
          let _ = ScopeStore::remove_scope_global(scope_name.as_str());
        }
      }
    }
  }

  fn impulse(&mut self, name: Ustr, #[optional] payload: serde_json::Value) {
    let flow_payload = FlowPayload::from_json(payload);
    let mut fields = HashMap::new();
    fields.insert("name".to_string(), name.to_string());
    fields.insert("payload".into(), format!("{flow_payload:?}"));
    self.impulse(name.clone(), Some(flow_payload.clone()), ctx.event_bus)?;
    let source = FlowInstance {
      name,
      payload: flow_payload,
      r#type: FlowType::Impulse,
    };
    let mut emitted = HashSet::new();
    let sm = ctx
      .registry
      .singleton_mut::<FacetGraph>(FacetGraph::KEY)
      .ok_or(CoreError::InvalidState(
        "state machine store not found".into(),
      ))?;
    log.log(LogLevel::Trace, "flow-runtime", "emitting signal", fields);
    self.reconcile_impulse_transcendence(sm, &source, ctx.event_bus, &mut emitted);
  }

  fn bootstrap(&mut self) {
    self.refresh_metadata_and_indexes(ctx.registry.metadata);
    self.setup_all_facet_subscribers(dispatch);
    ctx
      .registry
      .singleton_handle::<(&mut FacetGraph, &mut VariableHeap), _>(
        (FacetGraph::KEY.into(), VariableHeap::KEY.into()),
        |_, (sm, vh)| {
          let mut guard = HashSet::new();

          let existing_states = sm
            .facets
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
            )?;
          }

          self.reconcile_inverse_transcendence_all(
            sm,
            Some(vh),
            &mut guard,
            ctx.event_bus,
            dispatch,
          )?;
          self.save_facet_graph(sm)
        },
      )?;
  }
}

pub fn state_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_STATE_PATH") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/var/lib/system-state")
  }
}

pub fn state_root_path() -> PathBuf {
  if let Ok(path) = std::env::var("RIND_STATE_ROOT") {
    PathBuf::from(path)
  } else {
    PathBuf::from("/var/system-states")
  }
}

pub fn state_scope_path(scope: &str) -> PathBuf {
  if scope == "static"
    && let Ok(path) = std::env::var("RIND_STATE_PATH")
  {
    return PathBuf::from(path);
  }
  state_root_path().join(scope).join("state.bin")
}

pub fn condition_is_active(
  sm: &FacetGraph,
  cond: &FlowItem,
  payload: Option<&FlowPayload>,
) -> bool {
  for branches in sm.facets.values() {
    for branch in branches {
      let mut state = branch.clone();
      state.r#type = FlowType::Facet;
      if check_condition(cond, &state) && payload_compatible(payload, &state.payload) {
        return true;
      }
    }
  }
  false
}

pub fn condition_matches(
  sm: &FacetGraph,
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
  def: &FlowFacetMetadata,
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
  def: &FlowFacetMetadata,
  _payload: Option<&FlowPayload>,
  variables: Option<&VariableHeap>,
) -> Vec<FlowPayload> {
  let Some(cfg) = &def.auto_payload else {
    return vec![];
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
  def: &FlowFacetMetadata,
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

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct EmitTrigger {
  pub service: Option<Ustr>,
  pub name: Option<Ustr>,
  pub flow_type: Option<FlowType>,
  pub payload: Option<FlowPayload>,
  pub action: FlowAction,
  pub scope: Option<Ustr>,
}
