[[Rind]] is a self-contained, pluggable, extensible system runtime orchestrator with payload-aware, branchable state-and-signal flow trees, transport/IPC-driven control, integrated networking orchestration, layered user/permission management, and persistent runtime state.

## Init (or Main)
As soon as [[Rind]] starts up, the [[Boot]] process runs through a few integral stages.
- Plugin Discovery
- Orchestrator Mapping
- Unit Collection (with [[Extensions]])

## Env vars
[[Rind]] is configured via env vars.
- **`RIND_UNITS_PATH`**: *`"/etc/units"`*
- **`RIND_STATE_PATH`**: *`"/var/lib/system-state"`*