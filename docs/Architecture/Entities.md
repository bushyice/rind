An entity in [[Rind|rind]] is a model of a component. For example a [[Services|service]], [[Sockets|socket]] and alike are all entities, wrapped around a [[Units|unit]] and addressed via an [[#Address]].

## Address
Addresses are the full path of a component from a [[Units|unit]] in [[Rind|rind]]. They hold the full name of an entity.

**Example**: `rind:boot@static`
 - `rind` - the unit
 - `boot` - the entity
 - `@static` - the [[Scopes|scope]]

## Models
A model is a structure that's used for building an entity. A model is composed of [[#Metadata]] and [[#Instance]] data.

```rust
#[model(meta_name = name, meta_fields(name, exec, args))]
struct Service {
	pub name: String,
	pub exec: String,
	pub args: Vec<String>,
	pub state: ServiceState
}
```

- **Metadata fields**: `name`, `exec` and `args` are metadata fields, meaning they are immutable once loaded and remain as the configuration for this entity.
- **Name field**: the `meta_name` declares `name` as the name field, as such, that field will be used for the [[#Address]] of this entity.
- **Instance fields**: the `state` field is a mutable field that only exists at runtime and is needed for [[#Instantiation]] of the model.

## Metadata
[[Registry#Metadata|Metadata]] is immutable data about a model. Collected and stored once and only mutable initially within [[Orchestrators]].

## Instance
Instance data is the mutable runtime data of a model. After [[#Instantiation]], it can be mutated from any [[Runtimes|runtime]] with access to the [[Registry#InstanceRegistry|InstanceRegistry]].

## Instantiation
Instantiation is the process where a model is instantiated from metadata and the instance data:

```rust
Service {
	metadata: service_metadata,
	state: ServiceState::Inactive
}
```


## The `#[model]` Macro

The `#[model]` procedural macro generates a companion `*Metadata` type alongside the base struct. Metadata fields are immutable configuration loaded from TOML; instance fields are mutable runtime state.

```rust
#[model(
    meta_name = name,          // field used for the entity's Address name
    meta_fields(name, exec, args), // fields stored as immutable metadata
    derive_metadata(Debug, Default) // derives on the generated Metadata type
)]
pub struct Service {
    // Metadata fields (immutable after load)
    pub name: Ustr,
    pub exec: String,
    pub args: Vec<String>,

    // Instance fields (mutable at runtime, not in meta_fields)
    pub state: ServiceState,
    pub id: ServiceId,
}
```

This generates:
- `ServiceMetadata` struct containing only the metadata fields
- `Service` now holds an `Arc<ServiceMetadata>` accessible via its metadata
- The `Model` trait implementation for `Service`

See also: [[Units]], [[Registry]]
