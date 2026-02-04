# Kernel contract

Vizier now treats the kernel as the stable, pure domain layer shared by every frontend and driver.

## Purity target (v1)
The kernel is deterministic and side-effect free. It must not depend on:
- Filesystem access
- Git/VCS access
- Process spawning
- Network access
- TTY/stdout handling
- Async runtimes

## Kernel ownership
The kernel owns:
- **Config contract**: schema/types/defaults plus validation/normalization of a resolved config.
- **Prompt assembly** from preloaded context (snapshot + narrative docs + prompt templates).
- **Scheduler semantics** (types + `spec` decisions).
- **Audit/Outcome data types** (messages, audit state, narrative change sets, session artifacts).
- **Port traits** used by drivers/frontends to supply I/O.

## Frontend/driver ownership
Frontends and drivers own:
- Config resolution and precedence across TOML/flags/env/remote sources.
- File discovery and I/O (including loading repo prompt files and narrative context).
- Agent execution, VCS access, display/TTY output, and environment-specific UX.

## Current placement
- `vizier-kernel/`: kernel crate with pure logic.
- `vizier-core/`: driver host that wires kernel types to filesystem, VCS, and agent execution.
- `vizier-cli/`: user-facing frontend and CLI UX.

## Config handoff
Frontends resolve config sources, then hand the resolved config to the kernel for validation/normalization. The kernel does not read config files or inspect the environment directly.

## Related docs
- Port contracts: `docs/dev/architecture/ports.md`
- Driver responsibilities: `docs/dev/architecture/drivers.md`
