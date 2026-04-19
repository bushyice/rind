The state machine in [[Rind]] is the runtime store for active state instances. Persistence serializes that store to disk and restores it on boot.

## Runtime Store

The state machine is implemented in `crates/base/src/flow.rs`.

- `StateMachine.states`: `HashMap<String, Vec<FlowInstance>>`
- key: state full name (for example `unit@state`)
- value: branches/instances for that state name
- `StateMachine::KEY`: `runtime@state_machine`

`FlowInstance` stores:

- `name`
- `payload` (`json`, `string`, `bytes`, `none`)
- `type` (`state` or `signal`)

## Load/Save Boundary

`StateMachine` wraps a `StatePersistence` handle and exposes:

- `load_from_persistence()`: decode snapshot and rebuild `states`
- `snapshot_for_persistence()`: build snapshot for persistence

Persistence filtering rule:

- states whose name contains `@_` are skipped during snapshot save (impermanent state names).

## Persistence Format

- `StateSnapshot`: `HashMap<String, Vec<StateEntry>>`
- `StateEntry`: `{ data: Vec<u8> }` (bincode-encoded `FlowInstance`)
- file header:
- magic: `RIND`
- version: `u16` (current `1`)
- checksum: `u32` CRC32 of payload
- payload: bincode bytes for snapshot map

Writes are atomic-style:

1. encode snapshot
2. write to `path.tmp`
3. `sync_all` temp file
4. rename temp to target
5. sync parent directory

## Async vs Sync Save

`StatePersistence` supports both:

- `save(snapshot)`: async via internal channel + writer thread
- `save_sync(&snapshot)`: direct blocking write

Flow runtime currently calls `save` via `save_state_machine`, so normal state updates are async persisted.

## Boot Integration

Units orchestrator initializes state machine in preload.

- creates `StateMachine` singleton if missing
- creates `StatePersistence` using `state_path()`
- calls `load_from_persistence()` best-effort

Default state file path:
- env override: `RIND_STATE_PATH`
- fallback: `/var/lib/system-state`

## Runtime Actions That Mutate State

Flow runtime actions touching state/persistence:

- `set_state`
- `remove_state`
- `bootstrap` (reconcile existing states)

After mutation/reconcile, flow runtime saves snapshot.
