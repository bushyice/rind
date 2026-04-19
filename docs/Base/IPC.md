[[IPC]] is the operational interface to runtime control and observation from external processes to [[Rind]].

## IPC Responsibilities

- lifecycle operations (`start`, `stop`, `list`),
- signal/event submission,
- querying runtime state and service health,
- streaming lifecycle notifications.

## Security and Scoping

IPC enforce:

- caller identity resolution,
- [[Permissions|permission]] checks for mutating operations,
- user-space scoping for user-bound services,
- safe filtering of visible instances for non-privileged callers.
