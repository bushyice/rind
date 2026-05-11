---

kanban-plugin: board

---

## Todos

- [ ] **eBPF Loader**: (maybe?) Loading eBPF at system startup.
- [ ] [Trivial] **Name fixes**: Rename concepts accordingly for better understanding.
- [ ] **Dyn Units**: Units under the metadata `dyn-[XXXX]` that live isolated from the system units.
	- [ ] Dyn unit services/states/signals/...
	- [ ] Dyn unit registry.
	- [ ] Dyn plugins.
	- [ ] Dyn unit configs(isolation and options).
	- [ ] Dyn unit states and lifetime.
- [ ] **Shell entry**: A unified shell environment orchestrator that manages shell environment (e.g `devshell`).
	- **Fragments**: The units that hold data such as `env`, `bins`, `libs`.
	- **Resolvers**: Programs that translate configurations and inputs into sources.
	- **Sources**: Structured configurations that will be used to generate fragments.
	- **Providers**: Programs that turn sources into fragments.
	- **Spawners**: Generate a shell command and just pass it or execute a command directly.
	
	**e.g**:
	- NixShellEntry: Resolves `flake.nix`, provides with the nix provider to download and return from `/nix/store` before it finally just passing it to default spawner.


## Doing

- [ ] **Namespaces**: Service namespaces (user, network, mounts) in isolated envs.
	- [x] Basic namespaces
	- [ ] PID namespace proper `clone/fork+exec` flow (not `pre_exec+unshare`)
	- [ ] User namespace setup with `/proc/<pid>/uid_map`,`gid_map`,`setgroups`
	- [ ] Mount propagation setup
	- [ ] Rootfs isolation flow
	- [ ] Network namespace bring-up
	- [ ] Namespace persistence/join support
	- [ ] Namespace-local init/PID1 behavior (child reaping + sigfwd)
	- [ ] Capability bounding/drop pipeline
	- [ ] Seccomp profile (pre-exec)
- [ ] [CLEANUP] **Anyhow**: Remove all `anyhow` errors and results and move them to `CoreError` and `CoreResult`.
- [ ] **Sophisticated timers**
- [ ] **Memory Transport**
- [ ] **Json Optimizations**
- [ ] **KDL Configs**: Replace `TOML` with `KDL`.
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
- [ ] [BUG] **Notifier Inconsistency**: There's an inconsistency with notifiers where sometimes they do not notify. (e.g: When logging in and logging out).
- [ ] [BUG] **Session error**: 
	
	- [ ] There's an error where services stop when any user logs out, despite being logged in via other ttys.
	
	- [ ] There's an issue where logging out in any tty doesn't set the states (potential match operation issue)
	
	- [ ] There's an issue where login/logout have a race condition. and also sometimes user_login service starts and stops on-boot despite it seeing the login_required state alive and no remove_state requests.


## Testing

- [ ] **Service TP state piping address name for `branch_ctx`**
- [ ] **cgroups**: Using linux cgroups for service resource management.
- [ ] **Watchdog**: Service requirement to ping rind in order not to be terminated.
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
- [x] **Signal Branching**
- [x] [FIX] **State transcendence**: Check and fix state transcendence if it doesn't work.
- [x] **Networking as a plugin**: Move networking into a plugin to have more flexibility for a potentially optional(or replaceable) feature.
- [ ] **Permissions**: Entity-based(users, groups) access control for internal actions.
- [x] **TTY Manager Plugin**: A tty management plugin.
	- [x] `tty@active` state
	- [x] `tty@login_required` state/signal
	- [x] `tty_take` functionality
	- [x] `tty@switch` signal
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
