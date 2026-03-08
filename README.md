# `rind` (Rust Init Daemon)

> [!WARNING] 
> #### Disclaimer
> This repo contains a heap of objectively (and subjectively) bad code and is, in its entirety, designated POC/it-works-sometimes quality.
> It is currently in an experimental stage, and it has temporary parts inside the code that will be removed later.


`rind`(pronounced rin-dee, or rindy) is a simple init system written with rust, but with a few concepts that set it apart from something like systemd.

## Todo
- [x] **Core Architecture**: Unit loading, store management, ...
- [x] **Flow System**: Signal/State definitions and broadcasting.
- [x] **Payloads**: Typed support for JSON, String, and Binary data.
- [x] **Transport Protocols**: Transport protocols.
    - [x] `stdio`.
    - [x] `uds`.
    - [x] `env`.
    - [x] `args`.
- [x] **Service Management**:
    - [x] Process spawning and killing stuff.
    - [x] Dependency based startup (`after`).
    - [x] Restart polcies.
- [x] **State Branching**: Many state payloads at once.
- [x] **Service Branching**: Service per state branching.
- [x] **State Persistence**: Continuity of state across restarts.
- [x] **Detached Transports/Subscribers**: Independent messaging access for external programs.
- [ ] **Daemon & CLI**: The cli.
    - [x] Listing stuff.
    - [x] Enable/Disable/Start/Stop.
    - [ ] States and Signal control(maybe with permissions if those happen).
- [ ] **State Transcendence**: Auto-activation of states based on dependencies (e.g. `SwayActive` on `UserLoggedIn`).
- [ ] **Outputs**: Signal/State output collectd from services.
- [ ] **Piping**: Piping outputs and payloads into other states/signals.
- [ ] **Advanced Triggering**: More complex state based service triggers.
- [ ] **Userspace Isolation**: Isolate units for user and system.
- [ ] **Plugins**: Cycle-based internal programs with access to `rind`'s internal state.
- [ ] **Permissions**: Entity-based(users, groups, executables) access control for `Actions` and `ActionGroups`.
- [ ] **eBPF Loader**: (maybe?) Loading eBPF at system startup.


## Core Concepts
Look around, but most of these concepts are either there but untested or are still not made yet(eg. transcendence).

### Units: System Holders
Units are the basic __unit__ in rind, units serve as the definition point for all systems like `services`, `mounts`, and `flows`.

### Services: The actors
Services are the main parts of init systems such as systemd, as they are what __together__ build up the most complex systems as we know them. 

In `rind`, systems are declared under their parent(unit)'s namespace(eg. `unit@service`), and can be started and controlled in many ways.

```toml
[[service]]
name = "web-server"
exec = "/usr/bin/python3"
args = ["-m", "http.server", "8080"]
after = ["network-online"]

[[service]]
name = "tcp-server"
exec = "/usr/bin/tcp-server"
args = []
after = ["myunit@web-server"]
```

### Mounts: Probably useless
They do nothing besides being mount points and being mounted if activated. Namespaced as `unit@/target`
```toml
[[mount]]
source = "tmpfs"
target = "/tmp" # this is the identifier
fstype = "tmpfs"
create = true
after = "/dev"
```

### Flow: The main system
Alrighttttt, so this is kind of my favorite part because i came up with this while "fixing" a laptop for a friend of mine(i'll spare you the story). 

Basically, flow is a system in rind that:
  1) allows services to talk to each other
  2) makes the whole system dynamic
  3) allows a stateful system with dynamic service trees

There are two flow types:
  - **Signals**: Ephemeral, can only be one action type (apply).
  - **States**: Persistent, can be either applied or reverted.

Flows can have payloads, which is the actually cool part about them. 

Imagine this: `UserActive(username)` state and `LoadEnv(env)` signal for example. They're both system-wide, they can be global and they're isolated in their payload.

Services can also be dependent on signals or states, yeah like look at this shit:
```toml
[[service]]
name = "my-desktop-env"
exec = "/usr/bin/niri-or-some-shit"
args = []
start-on = ["user-active"] # or UserActive, i didn't decide on a format yet

[[state]]
name = "user-active"
payload = "json"
branch = ["username"]
# you could also gatekeep this state's broadcast units
broadcast = ["unit@some-service"]
```
And yeah, you can also access the payload from the service.


### Branching: uh... yeah
So, branching is basically having two payloads for one state. For example take `UserActive` state from earlier, you may have multiple users logged in through different `tty` in the system, and that's when you will need the branches. eg. `UserActive(makano, tty1)` and `UserActive(codebam, tty1)`.

However, services can also be branched to satisfy the multiple branches of states. Imagine `niri` from the example above, you can't run niri once and expect it to work for each `UserActive` branch, so you branch each service for each state branch. 

### Transcendence: did i spell that right?
So imagine if states can depend on other states, but also depending on the branches, wouldn't that be cool? then you could have `NiriActive(tty)` for each `UserActive` state. This way, you could have something like `DankShellActive(niri_pid)` state that depends on `NiriActive`, and you keep going until the whole thing is state-tree based. This allows you to link up whole systems and even make them profile-based.

__PS: I actually spelled it as "transcendance" before i wrote this readme, I searched up midway__

### Outputs: Who said whaaa?

So, let's say you ran a state through a service(when it starts for example), and let's say that service responded, but it doesn't have to put a new state into the pool just to respond, it can do that as an output(through transport protocols). For example when `UserActive` first starts, if the first service that starts is `user-env-manager`, instead of firing `LoadEnv()` it can just respond an output that will be attached to this state branch and can be accesed by every service that runs after `user-env-manager`.

__BTW: Everytime I use `{:?}` I whisper "whaaa" for some fucking reason__

### Piping: Okay maybe i need to stop?

Imagine a state having an output, and you wanna have that output in another state but as the payload. That's where piping comes in. You can pipe signals and states based on another state/signal's output or even payload.

### Transport Protocols: ...

So, we can input flow payload into services, but how? Well, here it is, transport protocols. The main transport protocol should probably be stdio which creates a private service-to-service messaging chain without the need for any external program or socket to handle this. But there's also UDS(unix domain sockets) which makes sense to have. Through either methods, though, we can communicate with the init system AND services to get this built-in messaging system.

There's also detached TPs, like UDS that are not attached to any service that can be defined by states, and that way any program can connect to such socket externally and use it as a messaging point.

## Upcoming Features

I am thinking of some cool(or useful) concepts that I might add if I get the thinking power to do so(one man can not do everything).

### Plugins

Imagine if you could make an init-integrated program that has direct access to the internal state(or maybe direct memory access?). I might implement this later on probably with WASM.

### eBPF

Idk

### Permissions

Could be possible? I mean you know what permissions are but it sounds tricky

### Fallback shell

Something that you get when your state is empty and no services are activated.

### Welp, that's it.

These are all the concepts i am planning to add, more might come up next time i fix someone's computer lmao.

## Requirements

- `qemu`
- `cpio`
- `gzip`

## Build System

You can build with the builder at `/builder`. It's a rust builder so you can just `cargo build` in the `builder` folder and copy the `builder` binary into the project root and use it. I personally copy it to `project_root/b` that way i can build with `./b ar`.

### Build Configuration

Inside of `builder.toml`, you can configure settings for how you want the builder to build and run the init.

## Build Commands

To get help, you can just execute the builder executable without any arguments and it will print help.

## Devenv

There's a flake.nix, but as of now it only builds and sets up the builder. So, once you do `direnv allow` you have the builder command available.

## Forsooth, With fellowship, we shall rise!

Any kind of help is appreciated! Granted, this may not be the most welcoming codebase, but you can look through or message me on anything and I will definitely respond!
