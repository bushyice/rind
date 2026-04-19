An **Orchestrator** is a policy-level component in [[Rind]] that defines system intent and manages the initialization of actuators for [[Runtimes]].

[[Orchestrators]] are responsible for setting up the environment, resolving dependencies, and triggering the initial logic for specific system domains.

Every orchestrator implements a standard trait that defines its lifecycle requirements and execution logic. An orchestrator is:

- identified by a unique string `id`.
- responsible for providing a set of [[Runtimes]] to the system.
- responsible for building [[Scopes]]
- executed in a deterministic order in it's respective [[Boot#Boot Cycles|Boot Cycle]] based on its dependency tree to ensure providers are initialized before consumers with access to [[Context]] and [[Runtimes#Runtime Dispatcher|Runtime Dispatcher]].