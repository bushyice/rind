Variables provide template substitution in [[Units]] and reusable run definitions. 

```toml
[[variable]]
name = "someservice"
default = { exec = "/bin/tcp", args = ["makano-test"], env = { PORT = "4533" } }
```


| Field | Type | Purpose |
|---|---|---|
| `name` | string | Unique variable name |
| `default` | table | Inline `{ exec, args, env }` definition |
| `env` | string | Environment variable name to source the definition from |

## Using Variables in Services

```toml
[[variable]]
name = "my-bin"
default = { exec = "/usr/bin/my-app", args = ["--port", "8080"] }

[[service]]
name = "app"
run.variable = "my-bin"        # references the variable above
```

## Environment-Sourced Variables

```toml
[[variable]]
name = "custom-path"
env = "RIND_CUSTOM_BIN"        # read from daemon environment at load time
```

## Default Structure

```toml
[[variable]]
name = "webserver"
default = {
    exec = "/usr/bin/httpd",
    args = ["-f", "/etc/httpd/httpd.conf"],
    env = { PORT = "80", LOG_DIR = "/var/log/httpd" },
}

```

## VariableHeap

The runtime store for variables. Variables are registered from unit definitions, then resolved at runtime with environment variable fallbacks.

```rust
pub struct VariableHeap {
    values: HashMap<Ustr, toml::Value>,
    defaults: HashMap<Ustr, toml::Value>,
    env_mappings: HashMap<Ustr, Ustr>,
    path: PathBuf,
}

impl VariableHeap {
    pub const KEY: &str = "runtime:variable_heap";
    pub fn new(path: impl Into<PathBuf>) -> Self;
    pub fn register(&mut self, id: impl Into<Ustr>, default: Option<toml::Value>, env: Option<Ustr>);
    pub fn set(&mut self, id: impl Into<Ustr>, value: toml::Value);
    pub fn get(&self, id: &str) -> Option<toml::Value>;
    pub fn get_full(&self, name: &Ustr) -> Option<(toml::Value, toml::Value)>;
    pub fn all(&self) -> impl Iterator<Item = (&Ustr, &toml::Value, toml::Value)>;
    pub fn contains(&self, id: &str) -> bool;
    pub fn load(&mut self) -> Result<Void>;
    pub fn save(&self) -> Result<Void>;
}
```

Variable resolution order: runtime values > environment variable > defaults.


See also: [[Services]], [[Units]], [[Scopes]], [[Orchestrators]]
