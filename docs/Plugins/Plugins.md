Plugins are dynamically loaded `.so` modules collected at `Collection` [[Boot#Boot Cycles|Boot Cycle]] and wired into [[Rind#Init|Main]].

### Plugin Trait Surface

A plugin implements:

- `get_metadata() -> PluginMetadata`: Plugin Metadata
- `provide_orchestrators() -> Vec<Box<dyn Orchestrator>>`: Plugin [[Orchestrators]]
- `unit_extension() -> Option<UnitExtension>`: [[Extensions|Unit extension]]

`PluginMetadata` fields:

- `name`
- `version`
- `deps`
- `caps`

### Loading Flow

At init startup:

1. collect plugin files from plugin directory
2. load `.so` with `libloading`
3. resolve `get_plugin` symbol
4. read optional `PLUGIN_ABI_VERSION` (defaults to `1` if missing)
5. call `get_plugin` and cache metadata
6. register orchestrators into boot engine
7. register optional unit extension into units orchestrator

### Path Resolution

`plugins_path()` currently resolves as:

- env `RIND_VARIABLES_PATH` if set
- else `/usr/lib/rind/plugins/`

### Authoring Macros

`rind_plugins::prelude` provides:

- `plugin!` macro to define plugin metadata + exported `get_plugin`
- `plugin_abi!(N)` macro to publish ABI constant

### Example shape
```rust
plugin!(
  name: "myplugin",
  version: 0,
  caps: PluginCapability::all(),
  deps: &[],
  create: MyPlugin,
  orchestrators: [MyOrchestrator],
  extension: myextension,
  struct MyPlugin;
);

plugin_abi!(1);
```
