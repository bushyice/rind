The [[Rind]] [[Boot]] process is managed by the `BootEngine`, which orchestrates the system's initialization through a multi-stage sequence of cycles and phases. It ensures that [[Orchestrators]] are executed in a deterministic order based on their dependencies and system-level initialization requirements.

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

