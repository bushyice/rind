![[units.png]]

[[Units]] are the definition points in [[Rind]]. [[Services]], [[States]], [[Signals]], [[Network Interfaces]], [[Variables]], [[Permissions]] and [[Mounts]] are [[Models]] defined via [[Units]].

[[Units]] also provide an extension mechanism: through [[Plugins]] and [[Extensions]], new [[Models]] can be introduced. These models are then registered in the [[Registry]] and instantiated, primarily via [[Runtimes]].

### Namespaces
Let's say you have `myunit.toml` in the `RIND_UNITS_PATH` path from [[Rind#Env vars]]. 

```toml
[[service]]
name = "myservice"
run.exec = "/bin/exec"
run.args = ["a", "b"]

[[state]]
name = "mystate"
payload = "string"
```

You can only access the [[Models]] defined in `myunit.toml` via `myunit@mystate` or `myunit@myservice`. Naming always uses `namespace@item` format.
