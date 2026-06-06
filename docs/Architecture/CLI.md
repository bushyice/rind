The CLI is the human interface. It communicates with the daemon via [[IPC]] and dispatches commands using structured messages.
## Commands

| Command        | IPC Action                               | Purpose                 |
| -------------- | ---------------------------------------- | ----------------------- |
| `start`        | `start`                                  | Start a unit            |
| `stop`         | `stop`                                   | Stop a unit             |
| `show`         | `show`                                   | Display unit/state info |
| `reload-units` | `reload_units`                           | Reload unit configs     |
| `logout`       | `logout`                                 | End user session        |
| `su`           | `run0`                                   | Escalate privileges     |
| `permission`   | `grant_permission` / `revoke_permission` | Manage permissions      |
| `invoke`       | *(user-provided)*                        | Send arbitrary action   |
| `scope`        | `create_scope` / `destroy_scope`         | Manage scopes           |
| `soft-reboot`  | `soft_reboot`                            | Soft-reboot the daemon  |
| `reboot`       | `reboot`                                 | Reboot the system       |
| `shutdown`     | `shutdown`                               | Shut down the system    |

## Command Details

Each command maps to one or more IPC actions:

- **`rind start <name>`**: sends a `start` request of a specified type
- **`rind stop <name>`**: sends a `stop` request of a specified type
- **`rind show <name>`**: sends `show`, returns unit metadata and current state
- **`rind reload-units`**: sends `reload_units`, triggers a Collect cycle
- **`rind su <cmd>`**: sends `run0`, escalates via privilege runtime
- **`rind logout`**: sends `logout`, ends the current session
- **`rind permission grant/revoke/show ...`**: manages ACL entries
- **`rind scope create/destroy ...`**: manages runtime scopes
- **`rind soft-reboot`**: sends `soft_reboot`, restarts runtime without exiting
- **`rind reboot`** / **`rind shutdown`**: system-level power operations

See also: [[IPC]], [[IPC#Transport Protocols|Transports]], [[Runtimes]]
