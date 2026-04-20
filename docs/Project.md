---
title: Welcome to rind
---
> [!CAUTION]
> **Experimental Status:** Rind is currently a proof of concept. The codebase is under active development and in a state of rapid flux. Expect architectural shifts, breaking changes, and occasional instability as we refine the core engine.

## Overview
**[[Rind]]** (Rust Init Daemon) is an init system and system runtime that provides the building primitives for reactive systems with persistent machine state, where services are drawn via dynamic state trees as opposed to static dependency threads. Built on a logic-driven **Flow** engine, Rind drives the system through a dynamic tree of persistent **[[States]]** and ephemeral **[[Signals]]**. This enables a truly fluid environment where services are actors that respond to real-time system facts, branching conditions, and structured data payloads.

---
## Features
- **Reactive Flow Engine**: A logic-driven runtime that maps ephemeral **[[Signals]]** and persistent **[[States]]** into dynamic service lifecycle actions.
- **Branchable States & Services**: Natively supports multi-instance behavior, allowing a single service to spawn unique instances based on state branches (e.g., a session manager per logged-in user).
- **State Transcendence**: Sophisticated dependency trees where states automatically activate or revert based on the presence and payloads of other states.
- **Payload-Aware Orchestration**: Services can consume, pipe, and transform structured data (JSON, String, Bytes) directly from the system state via integrated transport protocols.
- **Flexible Transport Protocols**: Built-in communication via `stdio`, Unix Domain Sockets (`uds`), environment variables, and command-line arguments for seamless service-to-init interaction.
- **Integrated Networking**: Network interfaces and configurations are treated as first-class states, enabling reactive service triggers based on live connectivity.
- **Persistent Runtime Facts**: System states can be configured to persist across reboots, ensuring continuity of the runtime environment.
- **Permission-Gated Control**: Entity-based access control for state mutations and signal emissions, ensuring secure interaction with the init system.
- **Pluggable Architecture**: Designed for extensibility with support for internal plugins and custom unit extensions to tailor the system to specific needs.
- **Advanced Lifecycle Hooks**: Trigger side-effects, emit signals, or run scripts automatically during service start, stop, or failure events.
