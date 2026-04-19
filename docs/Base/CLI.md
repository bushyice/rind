The [[Rind]] CLI is implemented as `rind` and uses subcommands to drive IPC actions and local log reading.

### Command Set

Top-level commands:

- `logout`
- `su <ARGS...>`
- `logs [options]`
- `list [NAME] [type flags]`
- `start <NAME>`
- `stop <NAME>`
- `invoke <NAME> <PAYLOAD>`
- `reload-units`
- `soft-reboot`
- `reboot`
- `shutdown`

### IPC-Oriented Commands

These commands send messages to [[IPC]] actions:

- `logout` -> `logout`
- `start` -> `start_service`
- `stop` -> currently sends `start_service` with `force` payload (current code behavior)
- `invoke` -> arbitrary action name
- `reload-units` -> `reload_units`
- `soft-reboot` -> `soft_reboot`
- `reboot` -> `reboot`
- `shutdown` -> `shutdown`

List behavior:

- `list` requests action `list` with unit type selector payload.
- output printers parse typed payloads (`unit`, `service`, `state`, `network`, `ports`).

### Run0 Flow (`su`)

`su` performs a run0 handshake:

1. send `run0`
2. if daemon requests input, prompt for root password
3. resubmit auth payload
4. on valid response, spawn requested command with uid/gid `0`

### Logs Command

`logs` reads `.rlog` segments from disk and filters locally written by [[Logger]].

Key options:

- `--dir` log directory (default `/var/log/rind`)
- `-l, --level`
- `--target`
- `--message`
- `--since <unix>`
- `--current` (restrict since current boot time)
- `--field KEY=VALUE` (repeatable)
- `-n, --limit`
- `-f, --tail`
- `--less`
- `--poll-ms`

Log decoding expects RLOG binary records (magic `RLOG`) and applies query matching in CLI.

### Examples

```bash
rind list -s
rind start myunit@web
rind invoke emit_signal '{"name":"myunit@activate","payload":"ok"}'
rind logs --target flow-runtime --current -f
```
