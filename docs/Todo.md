---

kanban-plugin: board

---

## Todos

- [ ] **Permission Inheritance**: If user has PermissionA and PermissionB inherits PermissionA, then user has PermissionB.
- [ ] **eBPF Loader**: (maybe?) Loading eBPF at system startup.


## Doing

- [ ] **Service Cleanup**: Clean service instances with `Exited` states.
- [ ] **Piping**: Piping and payloads into other states/signals.
	- [x] Simple circumstantial piping
	- [ ] General piping
- [ ] **Daemon & CLI**: The cli.
	- [x] Listing stuff.
	- [x] Start/Stop.
	- [ ] States and Signal control(maybe with permissions if those happen).
	- [x] Run0
	- [x] Logger
	- [ ] Permissions
	- [x] Invoke-IPC
- [ ] **Transport Protocols**: Transport protocols.
	- [x] `stdio`.
	- [x] `uds`.
	- [x] `env`.
	- [x] `args`.
	- [ ] `memory`
- [ ] **Plugins**: Cycle-based internal programs with access to `rind`'s internal state.
	- [x] Plugin loader
	- [x] Plugin base
	- [ ] Plugin regisry
	- [ ] Plugin index
	- [ ] Plugin caps


## Testing

- [ ] **Envs**: Loading `.env` files as user profile and as `rind` config source.
- [ ] **Reaper**: Zombie process terminator.
- [ ] **Userspace Services**: Isolate services for user and system.
- [ ] **Advanced Triggering**: More complex state based service triggers.
- [x] **State Transcendence**: Auto-activation of states based on dependencies (e.g. `SwayActive` on `UserLoggedIn`).
- [x] **Detached Transports/Subscribers**: Independent messaging access for external programs.
- [x] **Service Branching**: Service per state branching.
- [x] **State Branching**: Many state payloads at once.
- [x] **Payloads**: Typed support for JSON, String, and Binary data.


## Finished

- [ ] **Permissions**: Entity-based(users, groups) access control for internal actions.
- [x] **State Persistence**: Continuity of state across restarts.
- [x] **Flow System**: Signal/State definitions and broadcasting.
- [x] **Base Components**: Main unit components
	- [x] Models for units, services, mounts, states and signals 
	- [x] Auto service stopping
- [x] **Core Architecture**: Core system architecture
	- [x] Metadata and Models 
	- [x] Logger
	- [x] Errors
	- [x] Runtimes
	- [x] Contexts
	    - [x] Registries
	    - [x] Scope
	- [x] Events 
	    - [x] Dispatch 
	- [x] Orchestrators 
	- [x] Boot Cycles
- [x] **Service Management**:
	- [x] Process spawning and killing stuff.
	- [x] Dependency based startup (`after`).
	- [x] Restart polcies.




%% kanban:settings
```
{"kanban-plugin":"board","list-collapse":[false,false,false,false]}
```
%%