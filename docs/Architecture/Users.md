[[Users]] and sessions are first-class concepts in [[Rind]]. Internal runtimes manages login, logout, and user identity throughout the system via [[Facets]] and [[Scopes]].

## User Sessions via Facets

The built-in `rind:user_session` facet drives session lifecycle. It branches by TTY, and services can opt in via `start-on`:


```toml
[[service]]
name = "user-app"
run.exec = "/usr/bin/user-app"
space = "user"
start-on = [{ facet = "rind:user_session" }]
user-source = { facet = "rind:user_session", username-field = "username" }
```


## User Scopes

When a user session starts, the user orchestrator creates a per-user [[Scopes|scope]] automatically:

```
Scope "makano"
  attributes:
    user:       makano
    units_dir:  /home/makano/.local/share/units
  lifetime_state: rind:user_session
```

This means every user gets their own metadata namespace. Their services live at `makano:service-name@user-makano` rather than in the static scope. It's a poor man's container: separate metadata, separate state, separate lifecycle.


## UserContext

```rust
pub struct UserContext {
    pub record: UserRecord,
    pub groups: Vec<String>,
}

impl UserContext {
    pub fn new(record: UserRecord, groups: Vec<String>) -> Self;
    pub fn in_group(&self, group: &str) -> bool;
    pub fn is_root(&self) -> bool;
    pub fn is_privileged(&self) -> bool;
}
```

## UserSession

```rust
pub struct UserSession {
    pub id: u64,
    pub user: UserContext,
    pub tty: String,
    pub started_at: Instant,
}
```

See also: [[Orchestrators]], [[Context]], [[Flow]], [[Services]]
