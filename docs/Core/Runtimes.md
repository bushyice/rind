[[Runtimes]] are coordinated by the [[Rind]] main thread and run within a [[Context]], reacting to actions dispatched via the [[#Runtime Dispatcher]].

Every runtime implements a standard interface that defines its identity and how it processes commands. A runtime is:

- identified by a unique string `id`
- stateless in its definition, but manages stateful instances via the [[Registry]]
- executed sequentially within its domain to avoid lock-order coupling.

## Runtime Dispatcher

The runtime dispatcher is the messaging layer used to coordinate actions between different runtimes.

It is responsible for:

- routing `RuntimePayload` data to the correct runtime by id
- decoupling the caller from the target runtime's implementation,
- ensuring that actions are queued and executed within the correct [[Boot]] cycle.