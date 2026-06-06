Becoming is the hardest part of a system. [[Boot]] is the sequence of becoming, the structured transition from dead to alive. [[Philosophy/Continuity|Continuity]] starts at boot.


```mermaid
flowchart LR
    PB[PreBoot: mock runtime] --> CL[Collect: discover units]
    CL --> RT[Runtime: activate domains]
    RT --> PU[Pump: continuous]
```


## Boot Cycles

The boot sequence is divided into distinct execution cycles, representing the high-level stages of the system lifecycle:

- __Collect__: The discovery phase. [[Orchestrators]] use this to find metadata (e.g., loading [[Units]]) and prepare for runtime.

- **Runtime**: The primary initialization phase. This is where most system services and logic are activated.

- **PostRuntime**: Cleanup or finalization tasks that must occur after the primary runtime is established.

- **Pump**: A recurring cycle used for continuous synchronization or event processing after the initial boot is complete.

## Boot Phases

Each cycle (except **Pump** during normal operation) is further split into two phases to allow for "sandwiching" logic:

- **Start**: Executed in dependency order (top-down). Used to initialize resources that others depend on.

- **End**: Executed in _reverse_ dependency order (bottom-up). Typically used for finalization or teardown logic within a cycle.

## BootEngine

```rust
pub struct BootEngine {
    pub orchestrators: OrchestratorStore,
    next_context_id: usize,
    persistent_context_ids: Vec<usize>,
}
```

The engine runs three full cycles ([[#Collect]], [[#Runtime]], [[#PostRuntime]]) and then enters [[#Pump]]. For each cycle-phase pair it:

1. Allocates a unique context ID
2. Builds runtime scopes via `OrchestratorStore::build_scope_cycle_phase`
3. Registers scopes with the runtime handle
4. Runs orchestrators via `run_cycle_phase`
5. Flushes all queued runtime dispatch commands


```rust
pub fn run(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
    resources: &mut Resources,
) -> Result<Void, CoreError>;
```

## PreBoot

Before the first cycle, `pre_boot()` runs with a mock runtime handle. This means that [[Orchestrators]] can register metadata and instances before the real runtimes exist.

```rust
pub fn pre_boot(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    resources: &mut Resources,
    log: LogHandle,
) -> Result<Void, CoreError>;
```


## Pump Cycle

After boot, the Pump cycle runs continuously. It's the hook for all reactivity in the system. 

```rust
pub fn pump_once(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
    resources: &mut Resources,
) -> Result<Void, CoreError>;
```

## Reload

Units can be reloaded at runtime without full reboot. The engine removes all non-static metadata, clears indexes, then reruns the Collect cycle.

```rust
pub fn reload_units_collection(
    &mut self,
    metadata: &mut MetadataRegistry,
    instances: &mut InstanceMap,
    runtime: &RuntimeHandle,
    resources: &mut Resources,
) -> Result<Void, CoreError>;
```

## Dependency Ordering

The `OrchestratorStore` uses Kahn's algorithm (topological sort) for ordering:

```rust
pub fn planned_indexes(
    &self,
    cycle: BootCycle,
    phase: BootPhase,
) -> Result<Vec<usize>, CoreError>;
```

- Start phase: dependency order (a → b → c)
- End phase: reverse dependency order (c → b → a)
- Cycles are checked for conflicts (Runtime + PostRuntime is an error)
- Dependency cycles are detected and reported as errors

## LifecycleAction

System-level actions that runtimes can request via the [[Context#LifecycleQueue|LifecycleQueue]]:

```rust
pub enum LifecycleAction {
    ReloadUnits,   // re-run the Collect cycle
    SoftReboot,    // restart runtime without exiting
    Reboot,        // full system reboot
    Shutdown,      // power off
}
```

The boot engine processes these during the Pump cycle by calling the appropriate `LifecycleQueue::next()` and handling each action.


See also: [[Orchestrators]], [[Scopes]], [[Runtimes]], [[Context]]
