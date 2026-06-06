Permissions control what actions can be performed.
## PermissionId

```rust
pub struct PermissionId(pub u16);
```
## Permission

```toml
[[permission]]
name = "SystemServices"
id = 1000


[[permission]]
name = "FacetWrite"
id = 1001
links = [1000]
group = "admin"
```

| Field   | Type    | Purpose                                 |
| ------- | ------- | --------------------------------------- |
| `name`  | string  | Human-readable permission name          |
| `id`    | integer | Numeric permission identifier           |
| `links` | array   | Linked permission IDs (implicit grants) |
| `group` | string  | Group that holds this permission        |

## System Permissions

| Permission            | ID    | Purpose                          |
| --------------------- | ----- | -------------------------------- |
| `PERM_SYSTEM_SERVICES` | 1000 | Allows managing system services  |
| `PERM_LOGIN`          | 1001 | Allows user login                |
| `PERM_RUN0`           | 1002 | Allows running as root (uid 0)   |
| `PERM_NETWORK`        | 1003 | Allows network configuration     |

## PermissionStore

A runtime store for managing permissions with overlay grants/revokes per user and group.

```rust
pub struct PermissionStore {
    inner: Arc<RwLock<PermissionStoreInner>>,
    by_id: Arc<Mutex<HashMap<u16, Ustr>>>,
    by_name: Arc<Mutex<HashMap<Ustr, u16>>>,
    pub users: UserStoreShared,
}

impl PermissionStore {
    pub const KEY: &str = "runtime:permission_store";
    pub fn new(users: UserStoreShared) -> Self;
    pub fn user_has(&self, uid: u32, perm: PermissionId) -> bool;
    pub fn user_check(&self, uid: u32, expr: &PermissionExpr) -> bool;
    pub fn from_name(&self, name: &Ustr) -> Option<PermissionId>;
    pub fn group_has(&self, gid: u32, perm: PermissionId) -> bool;
    pub fn grant_user(&self, uid: u32, perm: PermissionId);
    pub fn grant_group(&self, gid: u32, perm: PermissionId);
    pub fn ungrant_user(&self, uid: u32, perm: PermissionId);
    pub fn ungrant_group(&self, gid: u32, perm: PermissionId);
    pub fn new_perm(&self, name: impl Into<Ustr>, id: u16) -> Result<PermissionId>;
    pub fn reg_perm(&self, perm: PermissionId, name: impl Into<Ustr>) -> Result<&Self>;
    pub fn all(&self, subject: Option<u32>, group: bool) -> Result<Vec<(u16, Ustr, Option<Ustr>)>>;
}
```

## PermissionExpr

Expression-based permission evaluation. Used for complex access control rules.

```rust
pub enum PermissionExpr {
    All,                        // matches everything
    Any(Vec<PermissionExpr>),   // logical OR
    Exact(Vec<PermissionExpr>), // logical AND
    Group(Ustr),                // matches a permission group
    Perm(PermissionId),         // matches a specific permission
    RootOnly,                   // only uid 0
}
```


See also: [[Scopes]], [[Users]], [[Variables]]
