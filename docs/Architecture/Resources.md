[[Resources]] is an FD-based resource manager that handles file descriptor lifecycle (watch, pause, resume, terminate) for the [[Runtimes]] event loop.

```rust
pub struct Resources {
    actions: HashMap<i32, ResourceAction>,
    fd: HashMap<i32, FdLoc>,
    flags: HashMap<i32, EpollFlags>,
    unwatched_fds: HashSet<i32>,
    watched_fds: HashSet<i32>,
    removed_fds: HashSet<i32>,
    paused_fds: HashSet<i32>,
}
```

## ResourceAction

Each registered FD is bound to an action that fires when the FD becomes ready:

```rust
pub struct ResourceAction {
    pub runtime: Ustr,
    pub action: Ustr,
    pub payload: Option<Box<dyn Fn(RuntimePayload) -> RuntimePayload>>,
}

impl From<(&str, &str)> for ResourceAction {
    fn from((runtime, action): (&str, &str)) -> Self;
}
```

## FdLoc

Owned file descriptor locations:

```rust
pub enum FdLoc {
    Owned(OwnedFd),
    Timer(TimerFd),
}
```

## API

```rust
impl Resources {
    pub fn register_resource(&mut self, res: i32);
    pub fn watch(&mut self, res: i32);
    pub fn action(&mut self, res: i32, act: impl Into<ResourceAction>);
    pub fn get_action(&self, res: i32) -> Option<&ResourceAction>;
    pub fn pause(&mut self, res: i32);
    pub fn resume(&mut self, res: i32);
    pub fn is_paused(&self, res: i32) -> bool;
    pub fn own(&mut self, res: i32, fd: impl Into<FdLoc>);
    pub fn terminate(&mut self, res: i32);
    pub fn flag(&mut self, res: i32, flags: EpollFlags);
    pub fn flags(&self, res: i32) -> EpollFlags;
    pub fn unwatched_fds(&self) -> Vec<i32>;
    pub fn removed_fds(&self) -> Vec<i32>;
}
```

## Lifecycle

1. **Register**: `register_resource()` adds an FD to the unwatched set
2. **Watch**: `watch()` moves it to the watched set for epoll processing
3. **Pause/Resume**: temporarily remove/restore from epoll without destroying
4. **Terminate**: remove from all sets and drop the action binding


See also: [[Runtimes]], [[Context]], [[Boot]]
