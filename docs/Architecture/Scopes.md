[[Scopes]] are how [[Rind]] separates concerns. It's the [[Branching]] of realms itself, They are attached to [[Registry#Metadata Pages|Metadata pages]] and where everything lives.

## Addressing

Every unit is addressed as `group:name@scope`. The `@scope` suffix picks which metadata namespace the unit lives in.

```text
example.toml ─── group = "example"
   └── [[service]] name = "web_ser" ───> example:web_ser@static
```

The default scope is `"static"`. it's implicit and usually omitted:

| Expression               | Resolves to                |
| ------------------------ | -------------------------- |
| `example:web_ser`        | `example:web_ser@static`   |
| `example:web_ser@static` | (same, explicit)           |
| `example:web_ser@makano` | A different scope entirely |


## The Static Scope

`"static"` is the built-in scope, loaded at boot from the system units directory (`/etc/rind/units/` or `RIND_UNITS_DIR`). It holds all system-level units and is always present, you can't create or destroy it. Built-in definitions like `rind:user_session` and `rind:boot` are added only to the static scope.


## Dynamic Scopes

Scopes other than `"static"` are dynamic, created at runtime via IPC or by the user orchestrator. Each dynamic scope has its own:

- **Units directory**: where [[Units|unit]] files live (defaults to the system dir)
- **Metadata registry**: separate from the static scope, same unit types
- **Facet persistence**: state stored at `{persistence_root}/{scope}/state.bin`
- **Lifetime**: can be tied to a facet state via `lifetime_state`

```toml
# example: per-user scope "makano" with its own units
# Created via: rind scope create makano --attr user=makano
# Loads units from a user-specific directory
```

## Scope Attributes
Scopes can have attributes that define how internal components such as [[Services]] behave. As an example, a scope with the attribute `"user"` will have all services inside of it as that user by default.



See also: [[Units]], [[Users]], [[Persistence]], [[Boot]], [[Orchestrators]], [[Runtimes]]
