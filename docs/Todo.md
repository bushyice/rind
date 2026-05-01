---

kanban-plugin: board

---

## Todos

- [ ] **eBPF Loader**: (maybe?) Loading eBPF at system startup.
- [ ] **cgroups**: Using linux cgroups for service resource management.
- [ ] **Namespaces**: Service namespaces (user, network, mounts) in isolated envs.
- [ ] **Watchdog**: Service requirement to ping rind in order not to be terminated.


## Doing

- [ ] **Memory Transport**
- [ ] **Json Optimizations**
- [ ] **Piping**: Piping and payloads into other states/signals.
	- [x] Simple circumstantial piping
	- [ ] General piping
	- [ ] Signal-to-state merging
- [ ] **Transport Protocols**: Transport protocols.
	- [x] `stdio`.
	- [x] `uds`.
	- [x] `env`.
	- [x] `args`.
	- [ ] `memory`
- [ ] **Daemon & CLI**: The cli.
	- [x] Listing stuff.
	- [x] Start/Stop.
	- [ ] States and Signal control(maybe with permissions if those happen).
	- [x] Run0
	- [x] Logger
	- [ ] Permissions
	- [x] Invoke-IPC
	- [ ] State-tree diagram
- [ ] **Plugins**: Cycle-based internal programs with access to `rind`'s internal state.
	- [x] Plugin loader
	- [x] Plugin base
	- [ ] Plugin regisry
	- [ ] Plugin index
	- [ ] Plugin caps


## Testing

- [ ] **Service TP state piping address name for `branch_ctx`**
- [ ] **Inverse Transcendence**: Branched and unbranched inverse transcendence (`activate_on_none`).
	- [x] Branched transcendence
	- [x] Unbranched transcendence
	- [ ] Auto Payload
	  - [x] With variables
	  - [ ] With commands
- [ ] **TImers**: Timers to trigger events after a preset duration.
- [ ] **Little Tasks 1**:
	- [x] Fix the persistent socket issue
	- [x] Test sockets <-> services <-> timers.
- [ ] **Sockets, FDs and timers**: 
	- [x] Socket-trigger-services
	- [x] FD resource manager
	- [x] Service timers
	- [x] Socket transcendence
	- [x] Socket branching
	- [x] Socket piping
	- [x] Socket state triggers
- [ ] **Permission Inheritance**: If user has PermissionA and PermissionB inherits PermissionA, then user has PermissionB.
- [ ] **Reaper**: Zombie process terminator.
- [ ] **String Optimizations**: Use something like `strumbra` for strings.
- [ ] **Userspace Services**: Isolate services for user and system.
- [ ] **Advanced Triggering**: More complex state based service triggers.
- [x] **State Transcendence**: Auto-activation of states based on dependencies (e.g. `SwayActive` on `UserLoggedIn`).
- [x] **Detached Transports/Subscribers**: Independent messaging access for external programs.
- [x] **Service Branching**: Service per state branching.
- [x] **State Branching**: Many state payloads at once.
- [ ] **Daemon Optimizations**: Replace loop with `epoll` to save wasted CPU cycles.
- [x] **Payloads**: Typed support for JSON, String, and Binary data.
- [ ] **Variables**: Dynamic definition values.
	- [x] As service run options
	- [x] As service pipes


## Finished

**Complete**
- [ ] **Permissions**: Entity-based(users, groups) access control for internal actions.
- [x] **Instance Deletion**: Remove items from instance registry.
	- [x] Socket Uninstantiation
- [x] **Envs**: Loading `.env` files as user profile and as `rind` config source.
- [x] **State Persistence**: Continuity of state across restarts.
- [x] **Trigger Optimizations**: Keep an index of flow <-> service relationships.
- [x] **Payload Optimizations**: Replace JSON with a faster payload for internal messaging.
- [x] **DI**: `ResourceBag` in place of json runtime payload.
- [x] **Flow System**: Signal/State definitions and broadcasting.
- [ ] **Service Optimization**: 
	- [x] Clean service instances with `Exited` states.
	- [x] PID-Service mapping for handling service instance checks (e.g. service quits).
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