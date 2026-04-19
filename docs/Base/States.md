![[state.png]]

[[States]] in [[Rind]] are flow condition definitions and runtime facts managed by the flow runtime. They are typed by `payload`, can be activated/removed, can branch into keyed instances, and can trigger other states/signals through `after` dependencies.
## Core Definition

The base state metadata defines identity and payload type.

- `name`: The unique state name.
- `payload`: Payload type. Valid values are `json`, `string`, `bytes`, `none`.

```toml
[[state]]
name = "active"
payload = "string"
```

## Activation on Missing Dependencies

![[inverse-transcendance.png]]

`activate-on-none` auto-manages a state when dependency states are absent.

- `activate-on-none`: List of state names.
- The state is auto-applied when **all** listed states are missing/empty.
- The state is auto-removed when **any** listed state becomes active.

```toml
[[state]]
name = "online"
payload = "none"
activate-on-none = ["net@interface"]
```

## Transcendent Dependencies

![[state-transcendance.png]]

`after` declares state conditions that drive this state automatically.

- `after`: List of `FlowItem` conditions.
- Runtime behavior:
- On source apply: this state applies when all `after` conditions match.
- On source revert: this state reverts when the condition set is no longer satisfied.

`FlowItem` forms:

- Simple: `"state-or-signal-name"`
- Detailed: `{ state = "..." }` or `{ signal = "..." }`
- Detailed can include `target` and `branch` match operations.

```toml
[[state]]
name = "net-configured"
payload = "json"
after = ["net@interface"]
```

```toml
[[state]]
name = "session-ready"
payload = "json"
after = [
  { state = "rind@user_session", target = { as = { authenticated = true } } }
]
```

## Branching and Payload Mapping

![[state-branching.png]]

`branch` controls how JSON payloads are keyed and mapped.

- For direct `set_state` on JSON states:
- `branch` fields define identity keys for upsert/merge.
- If omitted, default key is `id`.
- For transcendent (`after`) propagation:
- `branch` acts as mapping specs from source payload.
- Supports `target` or `target:source` form.

```toml
[[state]]
name = "user_session"
payload = "json"
branch = ["session_id"]
```

```toml
[[state]]
name = "service-user"
payload = "json"
after = ["rind@user_session"]
branch = ["username:user", "uid"]
```

## Auto Payload Generation

`auto-payload` generates payload when runtime needs a default/generated state payload.

- `auto-payload.eval`: Command to execute.
- `auto-payload.args`: Optional command args.
- `auto-payload.insert`:
- String key (single insert),
- List of keys (line-by-line mapping),
- `"root"` to use parsed output as root JSON.

```toml
[[state]]
name = "hostname"
payload = "json"
auto-payload = { eval = "/bin/hostname", insert = "value" }
```

## Transport Subscribers

States can publish set/remove events to configured [[Transport]] subscribers.

- `subscribers`: List of `TransportMethod` entries.
- On bootstrap, subscriber endpoints are set up.
- On state apply/revert, `type = "state"` messages are sent with action `set`/`remove`.

```toml
[[state]]
name = "net-interface"
payload = "json"
subscribers = [
  { id = "uds", options = ["/run/rind/net-interface.sock"] }
]
```

## Permission Gates

State writes through transport can be permission-gated.

- `permissions`: List of permission IDs (either `u16` or full name to [[Permissions]]).
- Used by transport incoming handling before dispatching `set_state`/`remove_state`.

```toml
[[state]]
name = "firewall"
payload = "none"
permissions = [1001, "myperm"]
```

## Broadcast
![[service-branching.png]]

- `broadcast`: Defined on the model and accepted in TOML.
- Current flow runtime path does not consume this field yet.
