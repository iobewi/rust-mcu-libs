# State → Indicator → WS2812

Qualifies the chain `iobewi-state` → `iobewi-indicator` → `iobewi-esp-ws2812` on the board's RGB
LED. It uses only the public API of the three crates and nothing of any product.

```text
scenario task ─ Wi-Fi source ─┐
                USB source  ──┤                ┌─ Observer ─ LED task ─ Indicator ─ WS2812
                              └─ Registry ─────┤
                                               └─ Observer ─ diagnostic task (per source)
```

Two simulated sources (`Wi-Fi`, `USB`) play the scenario below in a loop. Two independent consumers
observe the same registry:

- **LED**: maps the **aggregate** of the flags to a pattern and drives the indicator. It also wakes
  on the indicator's own deadlines (blink, pulse), so the animation goes on while no flag changes.
- **Diagnostic**: prints what **each source** did between two snapshots (`Snapshot::changes_since`).
  It knows nothing of the LED.

The mapping from flags to patterns (`look` in `src/main.rs`) belongs to this example: an error is
red blinking, else a connection is green steady, else a connection being made is blue blinking,
else an initialisation is white steady, else the LED is dark. It is not part of `iobewi-state` or
`iobewi-indicator`.

## Scenario

| Step | Action | LED | What it shows |
| --- | --- | --- | --- |
| 1 | Wi-Fi raises `Initializing` | white steady | |
| 2 | Wi-Fi `replace({Connecting})` | blue blinking | `replace` swaps the set in one revision |
| 3 | Wi-Fi `replace({Connected})` | green steady | |
| 4 | USB raises `Connected` | green steady, unchanged | the aggregate does not change: no LED write |
| 5 | Wi-Fi clears `Connected` | green steady, unchanged | USB holds it: the aggregate is the same, **the revision moved**, the diagnostic names the Wi-Fi, the LED is neither written nor restarted |
| 6 | USB clears `Connected` | off | |
| 7 | USB raises `Error` | red blinking | |
| 8 | USB clears `Error` | off | |

## Console trace

On the board's console (UART bridge). Each step prints its action, the registry revision and the
expected LED, then each consumer prints what it saw: the revision, the aggregate, the change of
each source, and the display decision with the number of LED writes it caused.

```text
STEP 5: Wi-Fi clears Connected | registry rev 6 | expected LED: green steady, unchanged: USB still holds Connected
[LED ] rev 7 aggregate {Connected} -> green steady (same pattern), LED writes +0
[DIAG] rev 7 aggregate {Connected}
[DIAG]   wifi Updated +{} -{Connected}
```

## Wiring and build

The LED pin is chosen by feature; no library holds a pin number.

| Feature | Board | LED pin |
| --- | --- | --- |
| `esp32s3` | ESP32-S3-DevKitC-1 v1.0 | GPIO48 |
| `esp32s3` + `devkitc-1-v1-1` | ESP32-S3-DevKitC-1 v1.1 | GPIO38 |
| `esp32c3` | ESP32-C3-DevKitM-1 | GPIO8 |

```sh
cargo +esp build -Z build-std=core,alloc --target xtensa-esp32s3-none-elf \
  -p example-state-indicator --features esp32s3 --release
```

## Execution

An Embassy executor (`esp-rtos`) runs three tasks. The registry uses `CriticalSectionRawMutex`.
The LED write is a blocking RMT transmission of one LED, a few tens of microseconds, so it does not
hold the executor. The tasks run on one core: this example does not qualify the dual-core wake-up
of `iobewi-state`.
