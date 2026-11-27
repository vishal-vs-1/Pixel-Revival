# Performance

Artwork conversion and game rendering are separate workloads. Cached art is reused; AI does not run for each displayed frame. These notes describe boundaries, not latency or smoothness guarantees.

## Current behavior

- Java ME can prepare known sources before play and admit bounded live work. Idle batches follow device memory and total source-pixel budgets; the former extra 32-image packed-catalog cap was removed.
- Inference uses the selected device policy; unavailable drivers/devices can change selection. A requested device is not proof of the device used. Inspect the recorded native device/timing report.
- NES defers idle artwork maintenance while playing. In-flight work may finish; capture and pose observation still cost time. There is no zero-overhead claim.
- Normal event/DB processing replaces periodic full original-image catalog recovery. Manual Sync performs the selected-game scan while players are closed.
- NES emulation/audio and display use separate workers with a two-frame display bound. Pose blending touches the replacement rectangle rather than converting the whole HD frame.
- Java ME startup preload is bounded by time, image count and bytes; later assets use normal lookup.

## Read timings correctly

| Measurement | What it includes |
|---|---|
| Native AI invocation | Process/device/model setup, temporary image I/O and inference |
| Saved/published batch | Source/job work, conversion, output validation/writing and publication as defined by that harness |
| Gameplay | Core, observation, replacement, presentation, scheduling and audio |

Compare only runs with matching device, model, input sizes, worker/batch settings and timing boundaries. Source size has a large effect. Stage durations can overlap and must not be added blindly.

Historical examples explain earlier apparent contradictions: a 256-image RTX 2050 run took 5.894 seconds with two processing workers; later AMD integrated-GPU stage runs took 27.719–35.803 seconds under different batching and cache conditions. A 37.491-second isolated total used another timing boundary. These are recorded observations, not current throughput targets. Full settings, size sweeps and failed trials remain in [EXPERIMENTS.md](EXPERIMENTS.md).

The newest fetched-sprite observer passed framebuffer-equivalence checks, which establish unchanged emulated output, not unchanged cost. Ordinary gameplay acceptance remains open in [Status](STATUS.md).

## Profiling tools

Use `tools/profile_enhancement_devices.py` for isolated device/batch measurements and `tools/check_ai_batch.py` for its defined end-to-end batch check. Consult their arguments before running and keep outputs under `build/`. Player stage stats distinguish observation and overlay costs. Optional expiring AI tracing records device arguments, batch timing and bounded native output locally.

Run profiling only when requested or authorized. No benchmark, recorder or game was run for this documentation cleanup.
