---

kanban-plugin: board

---

## Minor

- [ ] rind.env
- [ ] plguin index


## Major

- [ ] **eBPF Loader**: (maybe?) Loading eBPF at system startup.


## Testing

- [ ] **Permissions**: Entity-based(users, groups, executables) access control for `Actions` and `ActionGroups`.
- [ ] **Plugins**: Cycle-based internal programs with access to `rind`'s internal state.
- [ ] **Userspace Isolation**: Isolate units for user and system.
- [ ] **Advanced Triggering**: More complex state based service triggers.
- [ ] **Piping**: Piping and payloads into other states/signals.
	- [x] Simple circumstantial piping
	- [ ] General piping
- [x] **State Transcendence**: Auto-activation of states based on dependencies (e.g. `SwayActive` on `UserLoggedIn`).
- [ ] **Daemon & CLI**: The cli.
	- [x] Listing stuff.
	- [x] Start/Stop.
	- [ ] States and Signal control(maybe with permissions if those happen).
- [x] **Detached Transports/Subscribers**: Independent messaging access for external programs.
- [x] **State Persistence**: Continuity of state across restarts.
- [x] **Service Branching**: Service per state branching.
- [x] **State Branching**: Many state payloads at once.
- [x] **Service Management**:
	- [x] Process spawning and killing stuff.
	- [ ] Dependency based startup (`after`).
	- [ ] Restart polcies.
- [x] **Payloads**: Typed support for JSON, String, and Binary data.
- [ ] **Transport Protocols**: Transport protocols.
	- [x] `stdio`.
	- [x] `uds`.
	- [x] `env`.
	- [x] `args`.
- [x] **Flow System**: Signal/State definitions and broadcasting.
- [x] **Base Components**: Main unit components
	- [x] Models for units, services, mounts, states and signals 
	- [ ] Reaper
	- [ ] Auto service stopping
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


## Finished





%% kanban:settings
```
{"kanban-plugin":"board","list-collapse":[false,false,false,false]}
```
%%