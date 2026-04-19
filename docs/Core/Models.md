[[Rind]] [[Models]] define structure, rules, and lifecycle semantics for runtime entities. They are registered by core systems or [[Extensions]], stored as definitions, and instantiated by [[Runtimes]] (via the [[Registry]]) into concrete runtime instances.

[[Models]] are split into two portions, [[#Metadata]] and [[#Instance Data]]
## Metadata
This part of the model is the serializable data expected from `toml`. Once loaded, it is immutable and stored as `Arc<T>` and can only be modified by reloading from source. It can be accessed across [[Runtimes]] to create [[#Instance Data]] via [[Registry#Instantiate|Registry Instantiation]]. 

## Instance Data
Live runtime data owned/managed during execution.It is mutable and may be accessed by multiple [[Runtimes]] through controlled [[Registry]] APIs (not arbitrary shared mutation).

### Example
```rust
#[model(
  meta_name = name,
  meta_fields(
    name, id
  )
)]
pub struct MyModel {
  // Metadata
  pub name: String,
  pub id: u16,

  // Instance data
  pub status: String,
  pub active_sessions: usize
}
```

This would make two structs. `MyModel` and `MyModelMetadata`,
	where `MyModel:: M` is `MyModelMetadata`
		  `MyModel` implements `Model`
		  `MyModelMetadata` implements `NamedItem`
