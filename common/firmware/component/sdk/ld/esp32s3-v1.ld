/* Native Workload link layout for ESP32-S3, fixed-link, image format IWNI v1.
 *
 * The Agent reserves one RAM region (the tail of the reclaimed `dram2` area) and the
 * loader copies the image there. S3 internal SRAM1 is reachable twice:
 *   - instruction bus, 0x403xxxxx: code is *linked and executed* here;
 *   - data bus,        0x3FCxxxxx: the same bytes are *written/read* here
 *     (instruction bus address = data bus address + 0x6F0000).
 * Hence CODE lives at an instruction-bus address and DATA (rodata, data, bss) at a
 * data-bus address, in the same physical region:
 *
 *   CODE 0x403D3700 .. +0x5000  == dbus 0x3FCE3700 .. +0x5000   (20 KiB)
 *   DATA 0x3FCE8700 .. +0x3000                                  (12 KiB)
 *
 * These constants are the loader's `TargetLayout` for Target::Esp32s3 (see
 * iobewi-workload-image and docs/native-workload-image.md); an image linked elsewhere is
 * refused. No Agent symbol is referenced: the Workload only knows its context pointer.
 */
ENTRY(workload_entry)

MEMORY
{
  CODE (rx) : ORIGIN = 0x403D3700, LENGTH = 0x5000
  DATA (rw) : ORIGIN = 0x3FCE8700, LENGTH = 0x3000
}

SECTIONS
{
  .text : ALIGN(4)
  {
    *(.literal .text .literal.* .text.*)
    . = ALIGN(4);
  } > CODE

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
    . = ALIGN(4);
  } > DATA

  .data : ALIGN(4)
  {
    *(.data .data.*)
    . = ALIGN(4);
  } > DATA

  .bss (NOLOAD) : ALIGN(4)
  {
    *(.bss .bss.*)
    *(COMMON)
    . = ALIGN(4);
  } > DATA

  /DISCARD/ :
  {
    *(.eh_frame .eh_frame.* .comment .xtensa.info .debug_*)
  }
}
