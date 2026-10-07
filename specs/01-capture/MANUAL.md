# Capture manual check

Run per OS. Tick in the PR that lands changes to device selection.

Steps (each OS):
1. `cargo run -- --list-devices` lists the target output device (`out` row).
2. `SUBS_DEVICE="<out name>" cargo run` logs `capture started device=<out name>`.
3. Play audio through that device; samples are non-silent (subtitles appear / levels move).

- [ ] macOS (loopback, e.g. BlackHole as output device)
- [ ] Windows (WASAPI output device by name)
- [ ] Linux (PipeWire/Pulse; `.monitor` source or sink name)

## Stream error recovery
- [ ] macOS: run with `SUBS_DEVICE` unset, change the default output (or unplug headphones). Log shows `capture resumed` within 2 s and subtitles continue.
- [ ] Windows: same.
- [ ] Linux: same.
