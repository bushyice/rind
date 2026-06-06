Sockets are [[Rind|rind's]] communication endpoints, such as `UDS`, `TCP` or `UDP` sockets that keep active to be relayed into an owner [[Services|service]], treated as [[Resources]].


```toml
[[socket]]
name = "my_socket"
type = "uds"
listen = "/var/sock/my.sock"
```

| Field         | Type   | Purpose                                                                  |
| ------------- | ------ | ------------------------------------------------------------------------ |
| `name`        | string | Unique socket name                                                       |
| `type`        | string | `uds` (Unix domain socket), `tcp`, `udp`                                 |
| `listen`      | string | Path (uds) or address:port (tcp/udp)                                     |
| `owner`       | string | Owning service reference, e.g. `"group:service_name"`                    |
| `lifecycle`   | string | `managed` (daemon manages, default) or `owned` (service manages)         |
| `start-on`    | array  | [[Architecture/Flow#FlowItem\|FlowItem]] conditions to create the socket |
| `stop-on`     | array  | Conditions to remove the socket                                          |
| `on-start`    | array  | [[Architecture/Flow#Trigger\|Trigger]] actions on socket creation        |
| `on-stop`     | array  | Trigger actions on socket removal                                        |
| `trigger`     | array  | Trigger actions on incoming data/connection                              |
| `managed-by`  | array  | [[Permissions\|Permission]] names that can manage lifecycle              |
| `permissions` | array  | [[Permissions\|Permission]] names required to connect                    |

## Socket Types

```toml
[[socket]]
name = "unix-socket"
type = "uds"
listen = "/var/sock/app.sock"

[[socket]]
name = "tcp-socket"
type = "tcp"
listen = "0.0.0.0:8080"
```

## Lifecycle
Service lifecycle imples the lifecycle of the owner service. When `managed`, the owner service starts on it's own and `owned` means the owner service gets started when the socket triggers instead of waiting for the socket's events.

```toml
[[socket]]
name = "managed-socket"
type = "uds"
listen = "/var/sock/managed.sock"
lifecycle = "managed"
owner = "my-group:my-service"

[[socket]]
name = "owned-socket"
type = "uds"
listen = "/var/sock/owned.sock"
owner = "my-group:my-service"
lifecycle = "owned"
```

## Start and Trigger

Sockets, just like [[Services]], can start on conditions and trigger actions on incoming data:

```toml
[[socket]]
name = "ipcs_s"
type = "uds"
listen = "/var/sock/some.sock"
owner = "test:ipcs"
start-on = [{ facet = "net:configured" }]
trigger = [{ impulse = "test:thething", payload = "()" }]
```

When a client connects and sends data, the `trigger` actions fire.
## On-Start / On-Stop

```toml
[[socket]]
name = "api-socket"
type = "uds"
listen = "/var/sock/api.sock"
on-start = [{ impulse = "api:ready" }]
on-stop = [{ impulse = "api:stopped" }]
```


See also: [[Services]], [[IPC]]
