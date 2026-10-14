/* Historical IWNI v1 fixed RAM addresses. The default RAM sections stay separate. */
SECTIONS {
 .native_workload 0x3FCE3700 (NOLOAD) : ALIGN(4) {
  _native_region_start = .;
  . += 0x8000;
  _native_region_end = .;
 } > dram2_seg
}
ASSERT(_native_region_start == 0x3FCE3700, "IWNI RAM address changed")
ASSERT(_native_region_end == 0x3FCEB700, "IWNI RAM capacity changed")
ASSERT(_rwtext_end <= 0x403D3700, "Agent IRAM overlaps native component")
ASSERT(ADDR(.dram2_uninit) + SIZEOF(.dram2_uninit) <= 0x3FCE3700, "Agent reclaimed RAM overlaps native component")
