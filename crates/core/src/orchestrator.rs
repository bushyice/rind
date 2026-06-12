use std::collections::{HashMap, VecDeque};

use crate::context::ScopeBuilder;
use crate::error::CoreError;
use crate::prelude::Resources;
use crate::registry::{InstanceMap, InstanceRegistry, MetadataRegistry};
use crate::runtime::{Runtime, RuntimeActionSpec, RuntimeHandle, RuntimePayload};
use crate::types::Void;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BootCycle {
  PreBoot,
  Collect,
  Runtime,
  PostRuntime,
  Pump,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BootPhase {
  Start,
  End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrchestratorWhen<'a> {
  pub cycle: &'a [BootCycle],
  pub phase: BootPhase,
}

pub struct OrchestratorContext<'a> {
  pub context_id: usize,
  pub metadata: &'a mut MetadataRegistry,
  pub instances: &'a mut InstanceMap,
  pub runtime: &'a RuntimeHandle,
  pub resources: &'a mut Resources,
}

impl OrchestratorContext<'_> {
  pub fn registry(&mut self) -> InstanceRegistry<'_> {
    InstanceRegistry::new(&*self.metadata, self.instances)
  }

  pub fn dispatch(
    &self,
    runtime_id: impl Into<String>,
    action: impl Into<String>,
    payload: RuntimePayload,
  ) -> Result<Void, CoreError> {
    self.runtime.dispatch(
      runtime_id.into().as_str(),
      action.into().as_str(),
      payload,
      self.context_id,
    )
  }

  pub fn dispatch_typed<A: RuntimeActionSpec>(
    &self,
    payload: A::Payload,
  ) -> Result<Void, CoreError> {
    self.runtime.dispatch_typed::<A>(payload, self.context_id)
  }
}

pub trait Orchestrator: Send {
  fn id(&self) -> &str;
  fn depends_on(&self) -> &[&str];
  fn when(&self) -> OrchestratorWhen<'static>;
  fn build_scope(&mut self, _builder: &mut ScopeBuilder) -> Result<Void, CoreError> {
    Ok(Void)
  }
  fn preload(&mut self, _ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError> {
    Ok(Void)
  }
  fn run(&mut self, ctx: &mut OrchestratorContext<'_>) -> Result<Void, CoreError>;
  fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    Vec::new()
  }
}

#[derive(Default)]
pub struct OrchestratorStore {
  pub list: Vec<Box<dyn Orchestrator>>,
}

impl OrchestratorStore {
  pub fn push<O: Orchestrator + 'static>(&mut self, orchestrator: O) {
    self.list.push(Box::new(orchestrator));
  }

  pub fn insert<O: Orchestrator + 'static>(&mut self, index: usize, orchestrator: O) {
    self.list.insert(index, Box::new(orchestrator));
  }

  pub fn extend(&mut self, orchestrator: Vec<Box<dyn Orchestrator>>) {
    self.list.extend(orchestrator);
  }

  pub fn planned_indexes(
    &self,
    cycle: BootCycle,
    phase: BootPhase,
  ) -> Result<Vec<usize>, CoreError> {
    let mut selected = Vec::<usize>::new();
    for (idx, item) in self.list.iter().enumerate() {
      let when = item.when();
      if conflicting_cycles(when.cycle) {
        return Err(CoreError::InvalidState(format!(
          "orchestrator `{}` has conflicting cycles Runtime and PostRuntime",
          item.id()
        )));
      }
      if when.cycle.contains(&cycle) && when.phase == phase {
        selected.push(idx);
      }
    }

    let mut deps = HashMap::<usize, Vec<usize>>::new();
    let mut indegree = HashMap::<usize, usize>::new();
    let mut id_to_idx = HashMap::<String, usize>::new();

    for idx in &selected {
      id_to_idx.insert(self.list[*idx].id().to_string(), *idx);
      indegree.insert(*idx, 0);
      deps.insert(*idx, Vec::new());
    }

    for idx in &selected {
      for dep_id in self.list[*idx].depends_on() {
        if let Some(dep_idx) = id_to_idx.get(&dep_id.to_string()) {
          deps.entry(*dep_idx).or_default().push(*idx);
          *indegree.entry(*idx).or_default() += 1;
        }
      }
    }

    let mut queue = VecDeque::<usize>::new();
    for idx in &selected {
      if indegree.get(idx).copied().unwrap_or_default() == 0 {
        queue.push_back(*idx);
      }
    }

    let mut order = Vec::<usize>::new();
    while let Some(node) = queue.pop_front() {
      order.push(node);
      if let Some(nexts) = deps.get(&node) {
        for next in nexts {
          if let Some(incoming) = indegree.get_mut(next) {
            *incoming -= 1;
            if *incoming == 0 {
              queue.push_back(*next);
            }
          }
        }
      }
    }

    if order.len() != selected.len() {
      let cycle_ids = selected
        .iter()
        .map(|idx| self.list[*idx].id().to_string())
        .collect::<Vec<_>>();
      return Err(CoreError::DependencyCycle(cycle_ids));
    }

    if phase == BootPhase::End {
      order.reverse();
    }

    Ok(order)
  }

  pub fn run_cycle_phase(
    &mut self,
    cycle: BootCycle,
    phase: BootPhase,
    ctx: &mut OrchestratorContext<'_>,
  ) -> Result<Void, CoreError> {
    let plan = self.planned_indexes(cycle, phase)?;
    for idx in plan {
      let orchestrator = self
        .list
        .get_mut(idx)
        .ok_or_else(|| CoreError::InvalidState("orchestrator index out of bounds".to_string()))?;
      if cycle == BootCycle::Collect {
        orchestrator.preload(ctx)?;
      } else {
        orchestrator.run(ctx)?;
      }
    }
    Ok(Void)
  }

  pub fn build_scope_cycle_phase(
    &mut self,
    cycle: BootCycle,
    phase: BootPhase,
    builder: &mut ScopeBuilder,
  ) -> Result<Void, CoreError> {
    let plan = self.planned_indexes(cycle, phase)?;
    for idx in plan {
      let orchestrator = self
        .list
        .get_mut(idx)
        .ok_or_else(|| CoreError::InvalidState("orchestrator index out of bounds".to_string()))?;
      orchestrator.build_scope(builder)?;
    }
    Ok(Void)
  }

  pub fn runtimes(&self) -> Vec<Box<dyn Runtime>> {
    self.list.iter().flat_map(|x| x.runtimes()).collect()
  }
}

fn conflicting_cycles(cycles: &[BootCycle]) -> bool {
  cycles.contains(&BootCycle::Runtime) && cycles.contains(&BootCycle::PostRuntime)
}
