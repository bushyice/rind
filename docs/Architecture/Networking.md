[[Rind]] has internal basic networking that manages network interfaces.


```toml
[[network]]
name = "eth0"
method = "dhcp"
```

| Field | Type | Purpose |
|---|---|---|
| `name` | string | Interface name |
| `method` | string | `dhcp` (default) or `static` |
| `address` | string | Static IP with prefix, e.g. `"192.168.1.100/24"` |
| `gateway` | string | Default gateway |
| `dns` | array | DNS server addresses |

## DHCP

```toml
[[network]]
name = "eth0"
method = "dhcp"
```

## Static

```toml
[[network]]
name = "eth0"
method = "static"
address = "192.168.1.100/24"
gateway = "192.168.1.1"
dns = ["8.8.8.8", "8.8.4.4"]
```

## Namespace Networking

For namespace-isolated services, network configuration can be applied per-service:

```toml
[[service]]
name = "isolated"
run.exec = "/usr/bin/isolated"
namespaces = { net = true }
```

## NetworkRoute

Static routing rules for network interfaces:

```toml
[[network]]
name = "eth0"
method = "static"
address = "192.168.1.100/24"
route = [
    { destination = "10.0.0.0/8", gateway = "192.168.1.1" },
    { destination = "172.16.0.0/12", gateway = "192.168.1.1" },
]
```


See also: [[Services]]
