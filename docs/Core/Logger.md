The [[Logger]] is an async channel-based writer that emits both console output and binary segment files. It is shared across [[Runtimes]] through their [[Context]].

### Log Entry Model

Each log record is:

- `timestamp: u64` (unix seconds)
- `level: LogLevel` (`trace`, `debug`, `info`, `warn`, `error`, `fatal`)
- `target: String`
- `message: String`
- `fields: HashMap<String, String>`

Runtimes/orchestrators log through `LogHandle::log(...)`.

### Logger Config

`LogConfig` fields:

- `dir`
- `flush_interval`
- `segment_max_bytes`

Default config:

- dir: `/var/log/rind`
- flush interval: `250ms`
- segment max: `16 MiB`

### Write Pipeline

1. `start_logger` creates channel + logger thread.
2. logger thread receives `LogEntry` with timeout.
3. each entry is printed to stdout in formatted text.
4. each entry is encoded and appended to current `.rlog` segment.
5. writer flushes and rotates segment on size threshold.

### RLOG Record Format

Per-record binary format:

- magic `u32` = `0x524C4F47` (`RLOG`)
- total_len `u32`
- payload_len `u32`
- payload (bincode of `LogEntry`)
- crc32 `u32` (payload)

Segment files are named like `00000001.rlog`.

### Fallback Behavior

If segment open fails in configured directory:

- logger falls back to `/var/log/rind-fallback.rlog`.
