# Firmware domain identities

`Domain::A` and `Domain::B` retain IDs 0 and 1. Applications can assign additional
IDs with `Domain::new(id)`, for example 2 for C. There is no list of supported
hardware platforms or implied execution policy in this type.

Every domain has `Slot::Zero` and `Slot::One`; `FirmwareSlot::other()` stays in
the same domain. Physical labels, capacities and metadata belong to the layout
and platform composition. These types do not encode any persistent record:
OTM1/OTM2 and transactional boot metadata retain their historical bytes.
