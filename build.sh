#!/usr/bin/env bash
set -e

ARTIFACTS=".artifacts"
RUST_INIT="init"
RUST_STUBS=(rind example)
ROOTFS="$ARTIFACTS/rootfs"
MOUNT_DISK_PATH="$ARTIFACTS/mnt"
CPIO="$ARTIFACTS/rootfs.cpio.gz"
DISKIMG="$ARTIFACTS/rootfs.img"
BZIMAGE="$ARTIFACTS/bzImage"
USE_DISK_IMG=${USE_DISK_IMG:-0}
RUN=${RUN:-0}
KERNEL_URL="https://l4re.org/download/Linux-kernel/x86-64/bzImage-6.6.8"
BUSYBOX="$ARTIFACTS/busybox"
SERVICES="$ARTIFACTS/services"

mkdir -p "$ARTIFACTS"
mkdir -p "$MOUNT_DISK_PATH"

echo "[*] Building rind..."
cargo build --release --target x86_64-unknown-linux-musl
# cp target/release/$RUST_INIT "$ARTIFACTS/$RUST_INIT"
cp target/x86_64-unknown-linux-musl/release/$RUST_INIT "$ARTIFACTS/$RUST_INIT"



echo "[*] Preparing rootfs..."
rm -rf "$ROOTFS"
mkdir -p "$ROOTFS"/{bin,etc,dev,proc,sys}
cp "$ARTIFACTS/$RUST_INIT" "$ROOTFS/init"
for stub in "${RUST_STUBS[@]}"; do
  cp target/x86_64-unknown-linux-musl/release/$stub "$ROOTFS/bin/"
done
cp -r $SERVICES "$ROOTFS/etc/"

echo "[*] Initializing Devices..."
sudo mknod -m 666 "$ROOTFS/dev/null" c 1 3
sudo mknod -m 666 "$ROOTFS/dev/tty1" c 4 1
sudo mknod -m 666 "$ROOTFS/dev/console" c 5 1


if [ ! -f "$BUSYBOX" ]; then
    echo "[*] Downloading static BusyBox..."
    curl -L -o "$BUSYBOX" https://busybox.net/downloads/binaries/1.19.0/busybox-x86_64
    chmod +x "$BUSYBOX"
fi

echo "[*] Adding BusyBox to rootfs..."
cp "$BUSYBOX" "$ROOTFS/bin/busybox"

for cmd in clear sh ls mount echo cp mv shutdown rm mkdir touch cat ln; do
  ln -sf busybox "$ROOTFS/bin/$cmd"
done

if [ "$USE_DISK_IMG" -eq 1 ]; then
    echo "[*] Creating disk image..."
    qemu-img create -f qcow2 "$DISKIMG" 512M
    fallocate -l 512M "$DISKIMG"
    sudo mkfs.ext4 -F "$DISKIMG"
    sudo mount -o loop "$DISKIMG" $MOUNT_DISK_PATH
    sudo cp -r "$ROOTFS/"* $MOUNT_DISK_PATH/
    sudo umount $MOUNT_DISK_PATH
    echo "[*] Disk image created at $DISKIMG"
else
    echo "[*] Creating initramfs..."
    (cd "$ROOTFS" && find . | cpio -H newc -o | gzip > "../../$CPIO")
    echo "[*] Initramfs created at $CPIO"
fi

if [ ! -f "$BZIMAGE" ]; then
    echo "[*] Downloading prebuilt Linux kernel..."
    curl -L "$KERNEL_URL" -o "$BZIMAGE"
    echo "[*] Kernel downloaded at $BZIMAGE"
fi

if [ "$RUN" -eq 1 ]; then
	echo "[*] Launching QEMU..."
	if [ "$USE_DISK_IMG" -eq 1 ]; then
	    qemu-system-x86_64 \
	        -kernel "$BZIMAGE" \
	        -hda "$DISKIMG" \
	        -append "console=ttyS0 init=/bin/$RUST_INIT" \
	        -nographic
	else
	    qemu-system-x86_64 \
	        -kernel "$BZIMAGE" \
	        -initrd "$CPIO" \
	        -append "console=ttyS0"
	        # -nographic
	fi
fi
