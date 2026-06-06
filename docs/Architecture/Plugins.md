Plugins are [[Rind|rind's]] [[Componentization|components]] of specific extensible optional scopes of features. 

## Plugin Trait

```rust
pub trait Plugin {
    fn get_metadata(&self) -> PluginMetadata;
    fn provide_orchestrators(&self) -> Vec<Box<dyn Orchestrator>> {
        vec![]
    }
    fn register_extensions(&self, _extm: &mut ExtensionManager) {}
}

pub struct PluginMetadata {
    pub name: &'static str,
    pub version: u32,
    pub deps: &'static [&'static str],
    pub caps: PluginCapability,
}
```

## Custom Entity Types

Plugins can register new [[Entities#Metadata]] types via the `RegisterLoader` system, enabling custom [[Units|unit]] sections. 

## PluginCapability

Plugin capability is declared as a bitflag:

```rust
bitflags! {
    pub struct PluginCapability: u64 {
        const ORCHESTRATORS = 1 << 0;
        const RUNTIMES     = 1 << 1;
        const IPC          = 1 << 2;
        const EXTENSIONS   = 1 << 3;
        const EXTENSIBLE   = 1 << 4;
        const INITRD       = 1 << 5;
    }
}
```

## Metadata Registration
Plugins can introduce new [[Entities#Models|models]] via extensions.

```rust
#[model(
    meta_name = name,
    meta_fields(name, ...),
    derive_metadata(Debug)
)]
pub struct CustomEntity {
    pub name: Ustr,
    // ...
}
```

## Extension System

Extensions use an `ExtensionManager` stored in thread-local storage, dispatched by `TypeId`:

```rust
thread_local! {
    pub static EXTENSIONS: OnceCell<ExtensionManager> = OnceCell::new();
}
```


There are three extension variants:

```rust
pub enum Extension<T> {
    Enquire(ExtensionEnquire<T>),   // fn(name) -> Result<T>
    Act(ExtensionAct<T>),           // fn(name, &mut T) -> Result<void>
    Resolve(ExtensionResolve<T>),   // fn(name, T) -> Result<T>
}
```

Plugins register extensions via `ExtensionManager::register` and the host allows `ExtensionExecutionCtx` callbacks to return `Dispatch` actions.

## Extension Functions
```rust
// act
fn my_act_ext(action: &str, data: &mut TheData) -> CoreResult<()> {
  if action == "do_something" {
	  data.do_something();
  }
  Ok(())
}

// resolve
fn my_resolve_ext(action: &str, data: TheData) -> CoreResult<TheData> {
  if action == "do_something" {
	  data.do_something();
  }
  Ok(data) // or a new TheData
}

// enquire
fn my_enquire_ext(action: &str) -> CoreResult<TheData> {
  Ok(TheData::new(action)) 
}
```


## Plugin definition

Plugins explicitly declare metadata, [[Orchestrators]], and extensions in one place:

```rust
plugin! {
	name: "myplugin",
	version: 0,
	caps: PluginCapability::all(),
	deps: &[],
	create: MyPlugin,
	orchestrators: [MyOrchestrator],
	extensions: [resolve(inject_builtin), act(register_trigger)],
	struct MyPlugin;
};
```


See also: [[Orchestrators]], [[Runtimes]], [[Boot]], [[Entities#Metadata]]
