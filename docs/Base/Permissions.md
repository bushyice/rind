[[Permissions]] define capability policy for [[Rind]] runtime and service operations. They map identity context to allowed actions and protect mutation paths across runtime subsystems.

## Permission Layers

- runtime operations: who can start/stop/reload components,
- service operations: who can affect specific services or branches,
- data operations: who can mutate state/variables,
- resource operations: who can request mounts/network capabilities.

## Identity Inputs

Permission checks can use:

- caller identity from IPC/transport,
- runtime/system privilege context,
- resolved service user (`space`, `user-source`),
- model-level policy constraints.

## Permission Definitions

[[Permissions]] are either defined as [[Units]] or via the `Permission`
```toml
[[permission]]
name = "myperm"
id = 1010 # u16
```
##### Or
```rust
let permissions = ctx.registry.singleton_mut::<PermissionStore>(PermissionStore::KEY)?;
permissions.reg_perm(PermissionId(1010), "myperm")?;
```
