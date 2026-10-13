# Supervised-domain ESP composition

Reuses `iobewi-firmware-layout::session` for exclusive reservation and the shared
ESP `ArtifactWriteSession` for streaming. It contains no separate writer, boot
protocol, update engine or duplicate supervision state machine.

A real runtime supplies `DomainState::observe` with `Stopped` only after execution
is actually stopped, or `Running(slot)` after activation is actually complete.
`Unknown` fails closed. Use the same owner for all start/stop/activation actions.
`SlotLayout::for_b` retains historical B physical requirements; additional domains
can construct a checked layout with their own application-assigned domain ID.

This crate is the storage/reservation boundary. The historical OTM2 persistence
policy is available as `iobewi-firmware-update::supervised::SupervisedUpdate`,
including explicit begin/complete activation and recovery phases. Concrete
metadata flash access, digest readback and runtime start/stop/health wiring
remain a separate integration gate, with no mandatory copy of system A's boot
chain and no use of `otadata` by supervised domains.
