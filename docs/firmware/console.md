# Console connector and shared primary console (issue #102)

## Layers

| Layer | Crate | Role |
| --- | --- | --- |
| contract | `common/log/console` (`iobewi-console`) | `Console` byte sink, `ConsoleBackend`, the platform's primary console and its stamp. No hardware, no logger |
| connector | `arch/esp32/log/console` (`iobewi-esp-console`) | ESP32 transports: UART0, USB Serial/JTAG, none; optional UART1 secondary |
| application logger | `common/log/core` (`iobewi-log`) | levels, filtering, ring, distribution. Receives the connector as a `fn(&Record)`; unchanged |

The bootloader uses the connector directly (HALT, panic, diagnostics) and links neither `iobewi-log` nor the ring, the configuration manager or the network. The connector never depends on `iobewi-log`.

## Retained architecture (final state of #102)

| Image | Console | Why |
| --- | --- | --- |
| bootloader | **synchronous and direct**: connector -> ROM routine -> UART0. No ring, no asynchronous processing, no queue, no `iobewi-log`, no scheduler. HALT and panic lines are written before stopping | diagnostics must be readable and complete; speed is secondary (a DEBUG/TRACE bootloader may boot slower) |
| application | **`console-ring`** (feature `ring`, opt-in): 2 KiB static ring, whole messages, FIFO order, no allocation, whole-message drop with counter and marker on saturation (no producer ever waits), autonomous drain by the UART `TxDone` interrupt, bounded `flush`, panic path that prints the fatal line first and then replays what the ring still held when its state is provably consistent | Q2 showed that the direct path with a lock cannot keep messages whole and interrupts open at once (below) |
| application without `install` | the direct path (the `lock` build, still the default of `iobewi-esp-console`) | compatibility; **not** the retained application architecture |

`iobewi-log` keeps its responsibilities; the primary console remains a platform setting shared by both images.

## Why it exists

The second stage printed on USB Serial/JTAG (`esp-println/jtag-serial`) while the bench console and the application use UART0. The application used `esp-println` in `auto` mode, which picks the transport per write from a USB "start of frame" flag that never resets: the output of one board depended on whether a USB host had been seen.

## One setting for the platform

`IOBEWI_CONSOLE_PRIMARY` (`uart<N>`, `usb-serial-jtag`, `none`; default `uart0`) is read by `iobewi-console`'s build script, the same way for every image built in the platform's environment. It becomes `PRIMARY` and the stamp `IOBEWI-CONSOLE primary=<console>\0` that each image keeps.

- `iobewi-esp-console` fails the build if the variable names a console the chip cannot use as primary (`uartN`, N ≠ 0) or one other than the `primary-*` feature it was built with.
- `esp_console.py check <images>` (CI, after the bootloader and the OTA application are built) fails if an image has no stamp or if stamps differ.

Both images must therefore be built with the same environment; a build with a different value fails or is refused at packaging, instead of producing a silent board.

### Limits

- The check covers images built and packaged together. An application distributed on its own (OTA built elsewhere) carries a stamp, but nothing at run time compares it with the installed second stage: whoever packages it must run `esp_console.py check` against the bootloader of the target platform. An OTA must not be used to change the system console; that is a re-provisioning of the second stage.
- The ROM has its own console output, which this software does not control.
- UART0 is the ROM console: GPIO and baud rate are the ROM's, the connector does not configure them.

## Selecting another console

```sh
IOBEWI_CONSOLE_PRIMARY=usb-serial-jtag cargo build -p iobewi-esp-bootloader --no-default-features \
  --features esp32s3,console-usb-serial-jtag --profile bootloader
```

The application does the same with `-p example-firmware-ota-system --features esp32s3,firmware,console-usb-serial-jtag`.

## Panic and HALT

The primary console needs no state of ours: no logger, no allocator, no error handling (no circular dependency). **Observed on the bench**: bootloader lines on UART0 after `esp_hal::init`. **Not measured**: output before `esp_hal::init` (the ROM routine it uses does not depend on it, but this was not tested) and a forced bootloader HALT. Application panics with the ring were observed (Q4, below).

- Bootloader: `boot: PANIC` and its HALT reasons go through the connector. The panic location is not printed: keeping every panic site's file name costs about 2.5 KiB of a 32 KiB image.
- Applications: `iobewi_esp_console::panic_print(info)` writes the fatal line and nothing else on the direct path; with the ring installed it is the emergency path described below (fatal line first, then the replay). It does not use `iobewi-log`, so it works whether or not the logger is installed or filtering. `ota-system`, `https-client`, `https-health`, `config-manager`, `wifi-modes` and `state-indicator` use it; `indicator` and `ws2812` do not use the console and keep their silent handler.

## The direct path with a lock (not retained for concurrent applications)

Measured on the bench (Q2, ESP32-S3, two cores, 115200 baud): with `lock`, short lines (<= 64 B) stayed whole (800/800) but long lines interleaved massively (46/400 and 0/400 intact) and interrupts were delayed up to 7.3 ms (tick gaps 5.6-7.3 ms; mean producer write 8.9-35.9 ms); without the lock, lines were mixed from the short ones on while the tick stayed near 1 ms. Neither is acceptable, hence the ring. The rest of this section describes what the lock guarantees, for the applications that still use the direct path.


| | |
| --- | --- |
| bytes | never torn (application, `lock`): every chunk of at most 64 bytes is written under `esp-println`'s lock |
| short messages | a message of 64 bytes or less is never split |
| long messages | formatted in a 256-byte buffer (cut with `...`), written in chunks; another writer can interleave at a 64-byte boundary |
| interrupts | masked for the whole transmission of one chunk (64 B ≈ 5.6 ms at 115200), reopened for a few cycles between chunks; a long line keeps them masked most of its ~22 ms. esp-println flushes the UART every 32 bytes, so this is wire time, not FIFO time. Upstream `esp-println` (default features) masks for the whole `println!` line |
| interrupt context, panic | the lock is reentrant per core (not per context): no self-deadlock; a higher-level handler on S3 (level 6/7) can re-enter and interleave. On S3 a panic waits for the other core to release; if that core died holding the lock the message is lost until the watchdog resets |
| bootloader | no lock (one core, no interrupts) |
| `SecondaryUart` | critical section per non-blocking FIFO fill (at most 128 register writes), never while waiting for space; may interleave at fill boundaries; gives up after a bounded wait on a stuck UART. `flush()` does nothing: up to 128 bytes (≈ 11 ms) may still be in the FIFO when `write_bytes` returns |

Limits: nothing makes a multi-chunk message atomic against another task (that would mean holding interrupts off for the whole message, or a blocking lock that is unsafe in interrupt and panic context); a UART that never drains stalls the writer holding the lock, as before. Whether the truncated lines seen in the old bench capture came from missing locking (the old build used none) or from the `auto` transport was not established.

## Application console with a transmit ring (feature `ring`)

Q2 showed that no lock held while the UART transmits can both keep messages whole and keep interrupts open (an UART byte takes 87 µs; `esp-println` flushes synchronously). Applications can therefore use the ring instead of the direct path; **the bootloader never does** (synchronous, direct, no ring, no interrupt, no lock).

```text
producer ──deposit(whole message, µs)──▶ ring 2 KiB ──pump (FIFO fill ≤128 B)──▶ UART0 FIFO ──▶ wire
   task / interrupt                       │                  ▲
                                          └─ TxDone interrupt┘  (drains with no further log call)
```

* `iobewi_esp_console::install(UART0, tx_pin, config)` once, at start-up, on the core that serves the UART interrupt. Before it, the console is the direct one.
* **Saturation never makes a producer wait**: a message that does not fit is dropped whole, counted (`lost_messages()`), and a `WARN - console: N message(s) dropped` line comes first in the next deposit that fits. A message longer than the ring is dropped whole.
* **Three guarantees**: *deposit* (returned: in the ring whole, or dropped and counted), *transfer to the FIFO* (autonomous, in order, via `TxDone`, as long as that interrupt can run), *physical transmission* (`flush(timeout)`: ring empty and `TxDone` seen, thread context, bounded).
* Interrupt context: deposit is allowed (one short critical section, no waiting). The message leaves through `TxDone`.

### Concurrency (proved, not measured)

`common/log/console/src/ring.rs` documents the protocol and invariants. Summary: one actor at a time inside the ring (the critical section); `head` advances after a complete copy, `tail` only after the FIFO accepted the bytes; the panic path never takes the critical section and freezes the ring with a Dekker handshake (`busy`/`emerg`, `SeqCst` fences) before reading it. Proof by `loom` (exhaustive interleavings with the C11 memory model; two deliberately broken variants must fail) and by host tests (ordering, wrap, index overflow, saturation, loss marker, byte conservation, two-thread stress, freeze, abandon cases).

Disassembly (release `ota-system` with `console-ring`): ESP32-S3: `s32i busy; memw; l32i emerg` in the actors, `s32c1i` (claim) then `memw` before the freeze reads; ESP32-C3: `sw busy; fence rw,rw; lw emerg`, `fence rw,rw` in the freeze, `fence rw,w` before releasing stores and `fence r,rw` after acquiring loads.

### Panic

`panic_print` (ring installed): claim the emergency (one winner on S3), write `\r\nPANIC[core N]: …` directly (ROM routine, no lock, no ring), then confirm the freeze (waits at most 2 ms for an actor of the other core, polled by the timer with an iteration cap that is only a safety net), then replay the bytes never transferred, unchanged, between two `-- console ring … --` lines. No replay if: the panic happened inside the ring on this core, the freeze was not confirmed in time, or the ring state is doubtful (indices or witness); the fatal line is out in every case. A second simultaneous panic waits at most 25 ms for the winner's line, then writes its own; two emergency outputs can still interleave if that wait expires or if the second panic arrives during the replay, and a message whose start was already in the FIFO shows its end after the fatal line (the header says so). If the other core died inside the ring, the replay is abandoned after the freeze wait; if it died holding the critical section, nothing here waits for it. The ROM routine itself waits for the UART at wire speed: if the UART clock is dead it never returns (the watchdog, if armed, is the only recourse, as before).

### What is proved, measured, and limited by hardware (see also the qualification summary)

| Property | Status |
| --- | --- |
| whole messages, FIFO order, no torn message, loss counting, byte conservation, freeze/abandon logic | proved by tests and loom (host) |
| barriers actually emitted | disassembly (above) |
| UART `TxDone` drain without further logs, IRQ latency, critical-section durations, throughput, two cores, saturation | **hardware, Q3** (below) |
| `flush`, panic replay (P1), abandon when the panic is inside the ring (P2) | **hardware, Q4** (below) |
| UART0 interrupt routed to one core only | read in esp-hal 1.2.2 (`enable` maps for the current core) |
| ROM output wait, dead UART clock | hardware limit |

### Cost of the ring (release `ota-system`, `console-ring` against the default `lock` build)

| | ESP32-S3 | ESP32-C3 |
| --- | --- | --- |
| flash (`elf_size.py`) | +23700 B (4.0 %) | +18870 B (3.0 %) |
| `size`: text / data / bss | 589179 / 9900 / 462928 → 607855 / 14924 / 457904 | 871350 / 5764 / 194760 → 883804 / 7900 / 196904 |
| ring object (`RING`, zero-initialised) | 2088 B | 2088 B |

The flash growth is `esp-hal`'s UART driver (`Uart::new`, pin routing, interrupt handler: about 17 KB of text on S3) plus the ring and the emergency path (about 2 KB). The `data`/`bss` split moves by the same amount in both directions on S3 because `bss` is what remains of the linker's data region; the symbol-level DRAM cost is the ring (2088 B), the UART object (20 B) and a few flags, and about 5 KB of `.data` growth is **not attributed** (not `iobewi` symbols; probably `esp-hal` interrupt vectors). The bootloader is unchanged: image 29760 B, text+rodata 28203 B, no `ring` symbol. Stack: `console_print` and `panic_print` keep their 256-byte line buffer; the emergency path adds a second one (fatal line and replay headers). Stack high-water marks were not measured.

## Secondary consoles

See `arch/esp32/log/console/README.md`. The connector only moves bytes; fan-out remains the application's composition (`print` function passed to `iobewi_log::install`).

## What is validated how

| Property | Evidence |
| --- | --- |
| variable parsing, stamp, `Writer` | host tests of `iobewi-console` |
| accepted selections and rejected combinations (variable ≠ feature, `uart1`, unknown value, no feature, two features, bootloader/application mismatch), ESP32-C3 and S3 | `console_matrix.py`, CI step in both MCU jobs (a refusal must fail with its reason) |
| line formatting and truncation | host tests of `iobewi-console` (`LineBuf`) |
| stamp agreement bootloader/application | `esp_console.py` and its tests, CI step |
| output on the wire (UART0, USB Serial/JTAG, none), HALT/panic, ROM/bootloader/application ordering | **bench only** (not covered by software tests) |
| locking, interleaving, interrupt latency of the direct path | hardware, Q2 (limits above) |
| ring: atomicity, saturation, autonomous drain, latency, `flush`, panic replay and abandon | hardware, Q3 and Q4 (summary below) |
| UART1 secondary output | compiles only; needs a USB-UART adapter on the chosen pin, which the bench does not have |

## Qualification summary (ESP32-S3 DevKitC-1 N16, UART0/CH343 at 115200 baud; reports on PR #103)

Qualified on hardware only for the scenarios run; ESP32-C3 is covered by compilation, host tests and disassembly, **not** by hardware. Several of the images below are local probes (`console-probe`, not part of the repository) built on the PR tree; Q1/R1 used a local assembly of this PR and the geometry subset of #101.

| Campaign | Result |
| --- | --- |
| Q1 bootloader on UART0 | PASS: ROM and bootloader lines on CH343, handoff to `ota_0`, A `Stable`, `/health` 200 |
| R1 application on UART0 | partial PASS: `iobewi-log` -> `console_print` -> UART0 and `panic_print` -> UART0; Wi-Fi/HTTPS not exercised (image built without the OTA token/TLS identity, so it panicked by design) |
| Q2 lock vs no lock (two cores) | defects established (above): this is the comparison reference for the ring |
| Q3 ring | PASS: 400/400, 800/800 short, 800/800 long and 407/407 interrupt-context lines intact, no unexplained loss; saturation: 24 accepted + 76 dropped whole = 100, counter and marker; a line deposited from an interrupt left by itself; tick gap max 1.033 ms (core 0); producer write mean 109-116 us; longest ring section 65 us, longest drain step 115 us |
| Q4 `flush` | 125 calls, no early `true` (FIFO empty and transmitter idle at every return), timeout handled and transmission intact after it; one board, one baud rate: no universal proof |
| Q4 P1 | PASS: fatal line, 1222 bytes replayed = the header's count, 147 already sent + 1222 = 1369 deposited, no duplicate, no corruption, no hang |
| Q4 P2 | PASS for the abandon: panic inside the pump, fatal line out, replay abandoned, nothing replayed twice. The fault fired in the producer-side pump (inferred), **not** from the `TxDone` interrupt: the interrupt case is not qualified |

### Known limits (not blocking, kept explicit)

* ESP32-C3: no hardware qualification.
* Simultaneous panics on two cores: logic and `loom` only.
* A panic from interrupt context: not qualified directly.
* A panic during a deposit: software tests and `loom` only.
* Wi-Fi/application load was not part of Q3/Q4; interrupt latency was measured on core 0 only.
* The ROM routine behind the fatal line waits at wire speed and has no bound if the UART clock is dead (the watchdog, if armed, is the only recourse).
* `flush`: observed without early return over 125 runs; the meaning of `TxDone` is only as documented by the PAC.
* `fault-injection` is a test instrument (`arm_fault_in_pump`): off by default, not enabled by `ring`, `console-ring` or any example; only an explicit feature of a qualification image turns it on.
* The old truncated lines seen in the first bench capture were never attributed to a cause.

## Impact (ESP32-S3, measured; bootloader profile / release application)

| | before | after |
| --- | --- | --- |
| bootloader image (`.bin`) | 29808 B | 29760 B |
| bootloader `.data` / `.bss` | 1452 / 96 B | 1452 / 92 B |
| bootloader text+rodata (`size`) | 28251 B | 28203 B |
| application flash (`elf_size.py`) | 597975 B | 599079 B (+1104 B: panic message printing, `lock`, line buffer) |
| application `.data` / `.bss` (`size`) | 9892 / 462936 B | 9900 / 462928 B |

The stamp is about 30 bytes per image. `console_print` and `panic_print` put a 256-byte line buffer on the stack of the calling task (the panic handler runs on the panicking stack); stack high-water marks were not re-measured.
