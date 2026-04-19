[[Variables]] are mutable runtime values used for interpolation, parameterization, and control data that is not itself a flow state. They are loaded at [[Rind#Init]]

## States vs Variables

- [[States]] represent semantic runtime conditions used by flow transitions.
- [[Variables]] represent general mutable data used by runtime logic and transport mapping.
## Variable Use Cases

- dynamic command/runtime parameters,
- feature flags and knobs,
- computed values shared across services,
- temporary runtime context values.

## Lifecycle and Access

- variables may be global or scoped by runtime/unit/instance,
- mutation should use controlled APIs,
- reads in hot paths should avoid unnecessary cloning,
- updates should be atomic where multi-field consistency matters.

## Example

```toml
[[variable]]
name = "log_level"
default = "info"

[[service]]
name = "worker"
run.exec = "/usr/bin/worker"
transport = { id = "env", options = ["LOG_LEVEL=var:log_level"] }


[[service]]
name = "worker-2"
run.variable = "worker-var"
transport = { id = "env", options = ["LOG_LEVEL=var:log_level"] }
```
