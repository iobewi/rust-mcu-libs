# Firmware layouts and exclusive reservations

One portable partition validator plus the historical A/B layout constraints.
`validate_regions` checks the complete physical map, not just selected slots.
Platform readers own partition parsing and MCU-specific placement constraints.

`authorization::SlotLayout` checks two image regions and their protected metadata
region against flash capacity/erase geometry. `for_a` and `for_b` retain existing
constraints; `new` also supports additional application-assigned domains without
another engine. The caller must obtain regions from verified partition discovery.

`session::DomainState` represents one physical domain's execution observation:
`Unknown`, `Stopped`, or `Running(slot)`. An observation failure is never treated
as stopped. Verified stopped domains allow first installation. The system A
adapter separately requires its actual mapped running image.

`reserve` mutably borrows this owner until the guard is dropped. The writer then
mutably borrows the guard's opaque, non-clonable permit, preventing simultaneous
writers from that guard. Compile-fail doctests cover activation exclusion and
permit aliasing. This is not a hardware attestation: the application must own one
state for each domain and route every start/stop/activation through that owner.

The state/guard implements no persistence, runtime health policy, or rescue mode.
B/C restoration or reinstallation remains the supervised-domain composition's
responsibility, using the same common update engine and physical writer.
