# ESP32 firmware A layout

Hardware-neutral preflight of the ESP32 v1 firmware A slot mapping. It maps logical `Domain::A` slots to the existing physical labels `ota_0`/`ota_1` and checks `otadata` presence, alignment, flash bounds, overlap and duplicates. Firmware B remains optional and is deliberately not given an ESP bootloader partition here.

The library does **not** write flash, patch bootloader, persist OTM1 or activate images yet. Real OTA A requires integration with the existing shared flash mutex, a flash writer, image validation, transactional `otadata` write ordering and an EWBT-compatible custom bootloader. Passing these host tests alone is not an OTA end-to-end acceptance.
