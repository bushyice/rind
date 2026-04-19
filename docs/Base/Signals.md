![[state-inactive.png]]

[[Signals]] in [[Rind]] are typed event definitions emitted into the [[Flow]] runtime. A signal does not persist in the [[State Machine]]; it is emitted, validated against its payload type, and used to trigger dependent transitions.

## Core Definition

The base signal metadata defines identity and payload type.

- `name`: The unique signal name.
- `payload`: Payload type. Valid values are `json`, `string`, `bytes`, `none`.

```toml
[[signal]]
name = "activate"
payload = "string"
```

```toml
[[signal]]
name = "request_login"
payload = "json"
```

## Dependent Signal Chains

![[state-branching-inactive.png]]

Signals can trigger other signals via `after`.

- `after`: List of `FlowItem` conditions that must match.
- Runtime emits dependent signal only when all `after` conditions are active/matching.
- This is evaluated in `reconcile_signal_transcendence` after `emit_signal`.

```toml
[[signal]]
name = "request_logout"
payload = "json"
after = ["sessions@request_login"]
```

## FlowItem Matching (`after` Details)

`after` accepts simple or detailed condition entries.

- Simple: `"name"`
- Detailed:
- `{ state = "..." }` or `{ signal = "..." }`
- Optional `target` / `branch` operations:
- String equality, or
- option object: `{ binary = true }`, `{ contains = "..." }`, `{ as = { ... } }`

```toml
[[signal]]
name = "notify-admin"
payload = "json"
after = [
  { state = "backup-result", target = { as = { status = "failed" } } }
]
```

## Transport Subscribers


Signals can publish set/remove events to configured [[Transport]] subscribers.

- `subscribers`: Defined on signal metadata and parsed from TOML.
- Current flow runtime does not publish signal events through this field.
- Signal transport publication currently occurs through event/transport runtime paths, not `Signal.subscribers`.

## Permission Gates

Signal emission through incoming transport can be permission-gated.

- `permissions`: List of permission IDs (either `u16` or full name to [[Permissions]]).
- Checked in transport incoming handling before `flow.emit_signal` dispatch.

```toml
[[signal]]
name = "deactivate"
payload = "string"
permissions = [2001, "myperm"]
```

## Broadcast

- `broadcast`: Defined on the model and accepted in TOML.
- Current flow runtime path does not consume this field yet.

## Service Integration

![[service-state-dependence-inactive.png]]

Services emit signals through lifecycle hooks, and flow consumes them immediately.

```toml
[[service]]
name = "backup-job"
run.exec = "/usr/bin/backup"
on-stop = [{ signal = "backup@complete", payload = { status = "success" } }]
```
