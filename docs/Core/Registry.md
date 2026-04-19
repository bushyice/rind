The [[Rind]] global [[Registry]] owns and separates [[Models#Metadata|Metadata]] (immutable definitions) from [[Models#Instance Data|Instance Data]] (mutable runtime data), and offers controlled APIs to instantiate and access both.

## Metadata Store

A metadata store is a typed collection of model definitions. Each store groups metadata for one model family (for example, [[Units]] metadata). Metadata in a store is:

- loaded from source (`toml`, extensions, or internal registration),
- immutable after load,
- shared across [[Runtimes]] (typically via `Arc<T>`).

## Metadata Registry

The metadata registry is the registry subsystem that manages all metadata stores.

It is responsible for:

- registering model metadata stores,
- resolving metadata by model type and key,
- validating definition-level invariants before instantiation,
- exposing read-oriented lookup APIs.

## Instance Registry

The instance registry is the registry subsystem that manages live model instances.

It is responsible for:

- instantiating instance data from metadata,
- storing and retrieving active instances,
- enforcing runtime-safe mutation/access APIs,
- coordinating lifecycle-bound cleanup and disposal.


## Example

```rust
registry.meta::<ServiceMetaStore>().get("api")?;
registry.instances::<ServiceInstanceStore>().instantiate("api")?;
registry.instances::<ServiceInstanceStore>().with_instance_mut(id, |svc| {
    svc.restart_policy.bump_failures();
});
```