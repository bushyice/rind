A [[Scopes]] is a type-safe dependency injection container in [[Rind]] that holds runtime-global objects. Scopes are populated by [[Orchestrators]] during the [[Boot]] sequence and injected into [[Runtimes]] during execution.

## Runtime Scope

A [[#Runtime Scope]] is a collection of unique types used by a single runtime.

- **Type-Safe**: Values are stored and retrieved by their Rust `TypeId`.
- **Stateless Storage**: Allows [[Runtimes]] to access handles (like loggers or PAM sessions) without owning them.
- **Global Contribution**: [[Orchestrators]] can inject "Global" definers that apply to every runtime scope.

## Scope Builder

The [[#Scope Builder]] is the tool used by [[Orchestrators]] during the `build_scope` phase of the [[Boot]] cycle.

- **Isolation**: Ensures that each runtime only receives the specific types it needs.
- **Initialization**: Defers value creation until the scope is actually built for a specific execution phase.