root := ".artifacts/mnt"

mr:
  mkdir -p {{root}}
  sudo mount -o loop .artifacts/rootfs.img {{root}}

umr:
  sudo umount {{root}}

dr cmd: mr
  {{cmd}}

clean-state: mr
  sudo rm -rf {{root}}/var/lib/system-state
  just umr

build cmd:
  cargo run --manifest-path builder/Cargo.toml -- {{cmd}}
