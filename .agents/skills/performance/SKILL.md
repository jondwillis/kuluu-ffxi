---
name: performance
description: >
  Measure Kuluu frame delays, CPU or GPU costs, loading stalls, and performance regressions.
  Use for optimization work, task-pool changes, performance checks, benchmarks, or profiling configuration.
  Use verify for visible runtime evidence and retail-grounding before changing vanilla behavior.
---

# Measure performance before changing execution

Identify the delayed operation and the user-visible deadline.
For frame work, record the active limiter and frame budget.
Do not treat process CPU percentage as main-thread utilization.
Do not treat a visible mesh count as a draw-call count.

## Collect comparable evidence

Use an optimized build with an isolated target directory.
Keep the scene, camera, graphics settings, window size, and limiter fixed.
Separate startup, loading, and steady operation.
Finish builds before collecting a quiet baseline.
Record competing CPU work when testing system pressure.
Compare frame-time percentiles and intervals over budget, rather than average FPS alone.
An interval over budget does not prove a dropped presentation.
Record diagnostics overhead with the panel closed and open.

Use existing profilers instead of building a stack viewer.
On supported hosts, use `samply` for sampled Rust stacks and Firefox Profiler.
For a task-owned client, capture a bounded profile without command-line arguments:

```bash
samply record -p PID -d 5 -s --unstable-presymbolicate -o artifacts/verify/profile.json.gz
```

On macOS, run `samply setup` if attachment reports missing signing entitlements.
Use the permitted escalation procedure when needed.
Use `/usr/bin/sample PID 5 2 -file artifacts/verify/sample.txt` as a supported alternative.
If available, use Instruments Game Performance for CPU scheduling, waits, GPU work, and presentation.
Record an unavailable tool or denied capture without claiming a measured cause.

Kuluu already has a `trace` feature for Bevy system spans.
Build with `--features native-window,trace --release`.
The existing subscriber writes Chrome trace data on a clean application exit.
Open that data in Perfetto.
Keep recordings short because detailed traces can become large.
CPU span duration includes waits and time spent off the CPU.
Use scheduling evidence before attributing that duration to CPU execution.
Source: [Bevy profiling](https://docs.rs/crate/bevy/0.20.0/source/docs/profiling.md).

## Choose the execution method

First remove repeated work, unnecessary allocations, and unnecessary asset changes when measurements identify those costs.
Use Bevy queries, change detection, and run conditions for work that belongs in the ECS schedule.
Avoid unnecessary exclusive systems, resource conflicts, and ordering dependencies that serialize independent work.
Use `ComputeTaskPool` for parallel work that must finish within the current frame.
Use `AsyncComputeTaskPool` for CPU work whose result can arrive in a later frame.
Use `IoTaskPool` for work that mainly waits for I/O.
An `async` block does not make CPU work yield or move it to another thread.
Poll task completion without waiting for an unfinished task.
`block_on(poll_once(task))` is a nonblocking completion poll; do not reject it solely because of its name.
Bound concurrency, result application, upload work, and per-frame completion processing.
Check cancellation and cleanup across logout, disconnect, and zone changes.
Use [lifecycle symmetry](../bevy-lifecycle-symmetry/SKILL.md) for those boundaries.
Source: [Bevy task pools](https://docs.rs/bevy/0.20.0/bevy/tasks/index.html).

## Prevent regressions

Run the existing `scripts/checks.sh clippy` gate.
It includes standard Clippy performance warnings and the viewer blocking-call policy.
The policy rejects synchronous sleep, park, and thread joins in viewer code.
Use narrow exceptions only for an identified blocking worker, test, or capture driver.
Explain the permitted execution context in the exception reason.
The policy does not detect every blocking call or expensive algorithm.

For a measured hotspot, add a reproducible benchmark with a fixed input and a validated output.
Keep private assets and server credentials out of CI fixtures.
Prefer deterministic operation or allocation limits when those limits express the actual regression.
Use a controlled runner for timing thresholds.
Do not fail shared CI from a workstation FPS threshold or an unmeasured percentage allowance.
Report before-and-after distributions and remaining uncertainty.
Use [verify](../verify/SKILL.md) for changed visible behavior.
