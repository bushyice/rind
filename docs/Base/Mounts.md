[[Mounts]] in [[Rind]] are mount definitions managed by the `mounts` runtime. A mount entry describes one mount target and optional source/fs parameters, and can be ordered with dependencies.

## Core Definition

The core mount definition identifies where and what to mount.

- `target`: The mount target path (also the metadata key via `meta_name = target`).
- `source` (optional): Source device/fs name/path passed to `mount(2)`.
- `fstype` (optional): Filesystem type passed to `mount(2)`.

```toml
[[mount]]
source = "proc"
target = "/proc"
fstype = "proc"
```

## Mount Flags and Data

Mount options are controlled through `flags` and `data`.

- `flags` (optional): List of string flags mapped to `nix::mount::MsFlags`.
- `data` (optional): Raw data/options string passed to `mount(2)`.

Supported flag strings in current runtime:

- `MS_RDONLY`
- `MS_NOSUID`
- `MS_NODEV`
- `MS_NOEXEC`
- `MS_RELATIME`
- `MS_BIND`
- `MS_REC`
- `MS_PRIVATE`
- `MS_SHARED`
- `MS_SLAVE`
- `MS_STRICTATIME`
- `MS_LAZYTIME`

```toml
[[mount]]
source = "/srv/data"
target = "/data"
flags = ["MS_BIND", "MS_REC", "MS_RDONLY"]
```

## Target Creation

`create` controls directory creation for target path before mount.

- `create = true`: Runtime calls `create_dir_all(target)` before mounting.
- `create = false` or unset: Runtime does not create target path.

```toml
[[mount]]
source = "tmpfs"
target = "/tmp"
fstype = "tmpfs"
create = true
```

## Dependency Ordering

`after` delays mount until dependencies are mounted.

- `after` (optional): List of dependency mount IDs.
- Runtime mounts entries without `after` first, then resolves pending mounts when all dependencies are satisfied.
- Unresolved dependencies are logged as an error.

Dependency IDs are tracked as `unit@target` internally in `mount_all` ordering.

```toml
[[mount]]
source = "tmpfs"
target = "/run"
fstype = "tmpfs"
create = true

[[mount]]
source = "/run/rind"
target = "/var/run/rind"
flags = ["MS_BIND"]
after = ["init@/run"]
```

## Runtime Actions

The mount runtime exposes these actions:

- `mount`: Mount one metadata entry by name.
- `umount`: Unmount one metadata entry by name.
- `mount_all`: Mount all unit mounts with dependency resolution.
