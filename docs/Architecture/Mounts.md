Mounts manage filesystem mount points. 


```toml
[[mount]]
source = "proc"
target = "/proc"
fstype = "proc"
create = true

[[mount]]
source = "tmpfs"
target = "/tmp"
fstype = "tmpfs"
create = true
```


| Field            | Type   | Purpose                                                  |
| ---------------- | ------ | -------------------------------------------------------- |
| `source`         | string | Device, filesystem label, or pseudo-fs name              |
| `target`         | string | Mount point path                                         |
| `fstype`         | string | Filesystem type (`proc`, `sysfs`, `tmpfs`, `ext4`, etc.) |
| `flags`          | array  | Mount flags (`MS_BIND`, `MS_RDONLY`, etc.)               |
| `data`           | string | Mount options string                                     |
| `create`         | bool   | Create target directory if it doesn't exist              |
| `after`          | array  | Service dependencies                                     |
| `rind-broadcast` | bool   | Broadcast mount status changes                           |

See also: [[Services]], [[Flow]], [[Variables]]
