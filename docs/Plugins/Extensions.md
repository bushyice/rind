Extensions are unit-metadata hooks (`UnitExtension`) executed by `UnitsOrchestrator` during unit collection in [[Rind#Init|Main]].

### Extension Type

`UnitExtension` is:

- `fn(UnitExtensionAction) -> UnitExtensionAction`

Action variants:

- `Metadata(Metadata)`
- `BuiltIn(Metadata)`
- `LoadedUnits(Metadata)`
- `CreateIndex`

### Execution Points

Units [[Orchestrators|orchestrator]] invokes extensions in this order:

1. `Metadata` phase while constructing model schema registry
2. `BuiltIn` phase after built-in TOML is added
3. `LoadedUnits` phase after filesystem unit TOML load
4. `CreateIndex` phase after metadata insertion/index setup

### Typical Uses

- register new model arrays into metadata (`units.of::<MyModel>("themodel")`)
- inject built-in TOML blocks for default states/signals
- post-process loaded metadata before runtime indexing
- create plugin-specific indexes after metadata registration
