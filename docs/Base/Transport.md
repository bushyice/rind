[[Transport]] is the boundary mapping runtime data into process-consumable inputs. It defines how state/variable data reaches a service at execution time and serves as a communication point for services to [[Rind]].

## Common Transport Modes

- `env`: inject key/value pairs into environment.
- `args`: append/replace process arguments.
- `stdio`: stream I/O binding.
- socket-based modes: pass endpoints/addresses to service processes.

## State Injection Contract

Transport mappings should treat state references as contracts.

Example syntax:

- `state:db-config/connection-string`
- `state:active-session/username`

Contract concerns:

- missing path behavior,
- type conversion rules,
- default/fallback handling,
- validation and fail-fast policy.

## Example

```toml
[[service]]
name = "http-proxy"
run.exec = "/usr/bin/proxy"
transport = { id = "args", options = ["--upstream=state:rind@net-upstream"] }
```
