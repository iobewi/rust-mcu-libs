# Firmware slots

Portable, allocation-free identifiers for the existing A/B firmware layout with the two labels `ota_0` and `ota_1`. The layout contract `embewi-ab-v1` remains unchanged.

This crate defines slot names and indices, but deliberately contains no flash, boot selection, OTA lifecycle or network transport. Platform adapters map these names to flash partitions.

Host unit tests check the exact compatibility contract. Firmware image validation, transactional boot metadata and hardware qualification will be handled separately.
