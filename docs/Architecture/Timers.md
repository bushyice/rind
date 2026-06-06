[[Rind]] timers are one-shot delayed triggers. They fire a [[Flow#Trigger|Trigger]] after a duration.


```toml
[[timer]]
name = "timeout_four"
duration = "5s"
finish = [{ service = "group:four", stop = true }]
```


| Field      | Type   | Purpose                                                           |
| ---------- | ------ | ----------------------------------------------------------------- |
| `name`     | string | Unique timer name                                                 |
| `duration` | string | Duration like `"5s"`, `"3m"`, `"2h"`, `"1d"`                      |
| `after`    | array  | Service names that must be started first                          |
| `finish`   | array  | [[Architecture/Flow#Trigger\|Trigger]] actions executed on expiry |


## Duration Format

```toml
[[timer]]
name = "quick"
duration = "5s"

[[timer]]
name = "long"
duration = "2h"
```

## Finish Actions

When a timer expires, the `finish` triggers fire:

```toml
[[timer]]
name = "backup"
duration = "3600s"
finish = [{ impulse = "backup:run" }]
```

## After: Service Dependencies

Timers can wait for services to be active:

```toml
[[timer]]
name = "deferred-job"
duration = "30s"
after = ["database", "network"]
finish = [{ impulse = "job:start" }]
```

## Starting Timers from Triggers

Timers are typically started from [[Flow#Trigger|Trigger]] actions:

```toml
[[service]]
name = "worker"
run.exec = "/usr/bin/worker"
on-start = [{ timer = "timeout_four" }]
```

## One-Shot Only

Timers fire exactly once. To reschedule, emit the timer impulse again from the `finish` trigger:

```toml
[[timer]]
name = "periodic"
duration = "60s"
finish = [{ impulse = "periodic:tick" }, { timer = "periodic" }]
```

## Stopping Timers

```toml
# Start the timer
on-start = [{ timer = "my-timer" }]

# Stop the timer before it fires
on-stop = [{ timer = "my-timer", stop = true }]
```

See also: [[Services]], [[Flow]]
