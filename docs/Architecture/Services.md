Services are the primary component in [[Rind]]. They represent the processes managed by the system that essentially make up the system itself from a dynamic state tree via [[Flow]]. 

Services transition through: `Inactive` (default), `Starting`, `Active`, `Stopping`, `Exited(code)`, `Error`.


```toml
[[service]]
name = "my-app"
run.exec = "/usr/bin/my-app"
run.args = ["--port", "8080"]
restart = { max_retries = 3 }
```

| Field         | Type            | Purpose                                                                          |
| :------------ | :-------------- | :------------------------------------------------------------------------------- |
| `name`        | string          | Unique service name                                                              |
| `run`         | object / string | Execution configuration options for the service                                  |
| `after`       | array           | Service names that must start before this service                                |
| `start-on`    | array           | [[Architecture/Flow#FlowItem\|FlowItem]] conditions that trigger service startup |
| `stop-on`     | array           | Conditions that trigger service shutdown                                         |
| `on-start`    | array           | [[Architecture/Flow#Trigger\|Trigger]] actions executed when the service starts  |
| `on-stop`     | array           | [[Architecture/Flow#Trigger\|Trigger]] actions executed when the service stops   |
| `working-dir` | string          | Working directory path for the service process                                   |
| `space`       | string          | Isolation space (`system`, or `user`)                                            |
| `singleton`   | boolean         | Ensures only one instance of the service runs at a time                          |
| `user-source` | string / object | Source definition for the user context running the service                       |
| `transport`   | string / object | Communication channel configuration for data transport                           |
| `branching`   | object          | Branching based on a [[Flow#FlowItem\|\|FlowItem]]                               |
| `restart`     | string / object | Policy definition for automatically restarting failed services                   |
| `managed-by`  | array           | [[Permissions]] that can manage this service                                     |
| `cgroup`      | object          | Linux control group resource limits and constraints                              |
| `namespaces`  | object          | Linux namespace isolation settings (network, pid, mount, etc.)                   |
| `watchdog`    | object          | Health check and hang detection configuration                                    |


## Run Options

```toml
[[service]]
name = "basic"
run.exec = "/usr/bin/binary"
run.args = ["arg1", "arg2"]
run.env = { PORT = "8080", HOST = "localhost" }
```

Or using a variable reference:

```toml
[[variable]]
name = "my-bin"
default = { exec = "/usr/bin/binary", args = ["arg1"], env = { LOG_LEVEL = "debug" } }

[[service]]
name = "from-variable"
run.variable = "my-bin"
```

## Restart Policy

```toml
[[service]]
name = "ephemeral"
run.exec = "/usr/bin/ephemeral"
restart = false                     # never restart

[[service]]
name = "resilient"
run.exec = "/usr/bin/resilient"
restart = { max_retries = 5 }       # restart up to 5 times on failure

[[service]]
name = "always-up"
run.exec = "/usr/bin/always-up"
restart = true                      # always restart
```

## Start Conditions

Services start when `start-on` conditions are met (OR logic):

```toml
[[service]]
name = "backend"
run.exec = "/usr/bin/backend"
start-on = [
    { facet = "rind:user_session" },
    { impulse = "net:configured" },
]
```

## Stop Conditions

```toml
[[service]]
name = "backend"
run.exec = "/usr/bin/backend"
stop-on = [{ facet = "rind:shutdown" }]
```

## Service Stacking

```toml
[[service]]
name = "backend"
run.exec = "/usr/bin/backend"
after = ["database:daemon"]
```

## On-Start / On-Stop Actions

Side-effects triggered when a service starts or stops:

```toml
[[service]]
name = "backend"
run.exec = "/usr/bin/backend"
on-start = [
    { impulse = "notify:started", payload = "running" },
    { timer = "healthcheck:backend" },
]
on-stop = [
    { socket = "backend_socket", stop = true },
    { service = "dependent", stop = true },
]
```

Each action is a [[Flow#Trigger|Trigger]]: impulses, timers, services/sockets, scripts, or execs.

## Service Dependencies

```toml
[[service]]
name = "database"
run.exec = "/usr/bin/database"

[[service]]
name = "app"
run.exec = "/usr/bin/app"
after = ["database"]
```

## Branching

Multi-instance services driven by facet branches:

```toml
[[service]]
name = "user-shell"
run.exec = "/usr/bin/user-shell"
start-on = [{ facet = "rind:user_session" }]
branching = {
    source = "rind:user_session",
    key = "tty",
    max-instances = 16,
    except = ["facet:tty:taken"],
}
# optional transport to push a specific value
transport = { id = "env", options = ["DEMO_STATE=facet:$"] } # $ = branch facet, $/key = facet payload key
```

When `rind:user_session` gains a new branch, the service spawns a new instance. When the branch is removed, the instance stops.
## User Source

```toml
[[service]]
name = "user-service"
run.exec = "/usr/bin/user-service"
space = "user"
user-source = { facet = "rind:user_session", username-field = "username" }
```

Or from the branch key:

```toml
[[service]]
name = "user-service"
run.exec = "/usr/bin/user-service"
space = "user"
user-source = { branch = true, username-field = "username" }
```

## Service Space

```toml
[[service]]
name = "system-service"               # space = "system" (default)
run.exec = "/usr/bin/system"

[[service]]
name = "user-service"                 # space = "user"
run.exec = "/usr/bin/user"
space = "user"

[[service]]
name = "specific"                     # space = user selective
run.exec = "/usr/bin/specific"
space = { user = "makano" }
```

## Namespace Isolation a Cgroup Limits

```toml
[[service]]
name = "isolated"
run.exec = "/usr/bin/isolated"
namespaces = { mount = true, uts = true, ipc = true, net = true, cgroup = true }
```

```toml
[[service]]
name = "cgroup-demo"
run.exec = "/usr/bin/cgroup-demo"
cgroup = {
    path = "/sys/fs/cgroup/rind-demo/demo",
    memory-max = "128M",
    cpu-max = "50000 100000",
    pids-max = "64",
}
```

## Watchdog

```toml
[[service]]
name = "watched"
run.exec = "/usr/bin/watched"
watchdog = { interval-ms = 1000, grace-ms = 5000, action = "restart" }
```

Service sends periodic pings; if none arrives within the grace period, the action fires.

## Transport

Services communicate with the daemon via transport protocols:

```toml
[[service]]
name = "stdio-service"
transport = "stdio"

[[service]]
name = "shm-service"
transport = "shm"

[[service]]
name = "env-consumer"
transport = { id = "env", options = ["DEMO_STATE=facet:my-group:transport_state"] }

[[service]]
name = "uds-service"
transport = { id = "uds", options = ["detached=true"] }
```

## Singleton

```toml
[[service]]
name = "unique"
run.exec = "/usr/bin/unique"
singleton = true
```


## Executor

Services are spawned via pluggable executor backends. The `executor` field on `RunOption` selects which backend to use.

```rust
pub trait Executor: Send + Sync {
    fn name(&self) -> &'static str;
    fn spawn(&self, ctx: ExecutorContext) -> Result<Box<dyn InstanceHandle>>;
}

pub trait InstanceHandle: Send + Sync {
    fn pid(&self) -> Option<u32>;
    fn kill(&mut self, signal: Signal) -> Result<Void>;
    fn take_stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    fn take_stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>>;
}
```

| Executor         | Name       | Description                                   |
| ---------------- | ---------- | --------------------------------------------- |
| `NativeExecutor` | `"native"` | Standard fork/exec process spawning (default) |
| `RemoteExecutor` | `"remote"` | Remote process spawning over network (stub)   |
| `ImaExecutor`    | `"ima"`    | In-memory application executor (stub)         |

```toml
[[service]]
name = "custom-exec"
run.exec = "/usr/bin/custom"
run.executor = "native"
```

## ServiceId

Each service instance gets a unique atomic ID at runtime:

```rust
pub struct ServiceId(u64);
```

## ChildInstance

Tracks a running instance of a branching service:

```rust
pub struct ChildInstance {
    pub key: Ustr,
    pub user: Option<Ustr>,
    pub handle: Option<Box<dyn InstanceHandle>>,
    pub state: ServiceState,
    pub retry_count: u32,
    pub stop_time: Option<Instant>,
    pub manually_stopped: bool,
}

pub struct ChildInstanceGroup(pub Vec<ChildInstance>);
```

## Namespace Isolation

Services can be isolated via Linux namespaces:

```rust
pub struct ServiceNamespaces {
    pub mount: bool,
    pub uts: bool,
    pub ipc: bool,
    pub net: bool,
    pub pid: bool,
    pub user: bool,
    pub cgroup: bool,
    pub mount_private: bool,
    pub rootfs: Option<Ustr>,
    pub hostname: Option<Ustr>,
    pub persist: bool,
    pub init: bool,
}
```

### Capability and Seccomp Policies

```rust
pub struct ServiceIsolation {
    pub scope: Option<Ustr>,
    pub cgroup: Option<ServiceCgroup>,
    pub namespaces: Option<ServiceNamespaces>,
    pub capabilities: Option<CapabilityPolicy>,
    pub seccomp: Option<SeccompPolicy>,
}

pub struct CapabilityPolicy {
    pub drop: Vec<Ustr>,
    pub keep: Vec<Ustr>,
}

pub struct SeccompPolicy {
    pub profile: Option<Ustr>,
    pub path: Option<Ustr>,
}
```


See also: [[Runtimes]], [[Flow]], [[Mounts]], [[Sockets]], [[Architecture/Networking|Networking]], [[Context]]
