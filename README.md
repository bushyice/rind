# `rind` (Rust Init Daemon)

> warning: still empty and a work in progress

A simple init system written with rust.

## Requirements

- `qemu`
- `cpio`
- `gzip`

## Build System

You can build with the builder at `/builder`. It's a rust builder so you can just `cargo build` in the `builder` folder and copy the `builder` binary into the project root and use it. I personally copy it to `project_root/b` that way i can build with `./b ar`.

### Build Configuration

Inside of `builder.toml`, you can configure settings for how you want the builder to build and run the init.

## Build Commands

To get help, you can just execute the builder executable without any arguments and it will print help.
