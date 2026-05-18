# Initramfs Handoff (Boot Test)

If you boot `rind-init` from initramfs (`rdinit=/usr/bin/init`), the initramfs plugin can mount and switch to the real root before normal boot.

Set these env vars in your initramfs env:

```sh
# optionally, you could set this
# to where initramfs-specific plugins are loaded from
# if you're not using plugins that cross both boundaries
export RIND_INIT_PLUGINS_PATH=/lib/rind/plugins/initramfs

# real root mount config
export RIND_INITRAMFS_REAL_ROOT=/dev/vda
export RIND_INITRAMFS_REAL_ROOT_FSTYPE=ext4
# export RIND_INITRAMFS_REAL_ROOT_DATA="rw"
# export RIND_INITRAMFS_REAL_ROOT_READONLY=1

# optional mount target before pivot_root
export RIND_INITRAMFS_NEW_ROOT=/newroot
```

Handoff modes:

```sh
# switch_root and continue in-process
export RIND_INITRAMFS_HANDOFF_MODE=continue

# switch_root then short circuit and exec real init
export RIND_INITRAMFS_HANDOFF_MODE=exec
export RIND_REAL_INIT=/usr/bin/init
# currently useless
# export RIND_REAL_INIT_ARGS="--some-arg" 
```

Keep in mind:
- If `RIND_INITRAMFS_REAL_ROOT` is unset, the initramfs plugin skips switch-root.
- `exec` mode is probably better since it opens the real init anew.
