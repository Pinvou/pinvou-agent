# Engine cold-start observation

Before the first send, Pinvou prepares the runtime model, materializes Skills, creates the Engine, and loads and syncs the session. Do not add prewarming based on one perceived delay; first collect cold and warm samples from the local timing sidecar, then decide where the bottleneck is and what to optimize.

## Collecting samples

Observations are written automatically during normal conversations to `~/.pinvou3/sessions/<session-id>/timing_events.jsonl` and are never uploaded. Records contain only phase durations and a `cold`, `rebuilt` or `reused` kind; they contain no messages, models, paths, credentials or raw errors.

- `cold`: the session had no usable Engine and one had to be created from scratch.
- `rebuilt`: an existing Engine was rebuilt because the model or MCP configuration changed.
- `reused`: the existing Engine was reused.

New sessions, the first send after an app restart, and scheduled tasks naturally produce cold samples; consecutive sends in the same session produce reused samples. Idle Engines may be reclaimed after 30 minutes, so the next send also produces a cold sample. There is no need to restart the app repeatedly to gather samples.

Sidecars written by older versions have no `engine_startup` field, and the report ignores those records. After upgrading to a version that includes this observation, collect at least 30 cold and 30 reused samples:

```bash
cd pinvou3-app
npm run perf:engine-startup
```

Use `--input <sessions directory or a single timing_events.jsonl>` to analyze specific data and `--json` for machine-readable output. `--min-samples`, `--cold-p95-ms` and `--min-improvement-percent` adjust the experiment thresholds; invalid or missing numeric values fail immediately instead of producing a verdict.

## Interpretation

By default, cold start is only confirmed as worth a prewarming experiment when all of the following hold:

1. cold and reused each have at least 30 samples;
2. cold Engine acquisition P95 exceeds 300 ms;
3. the gap between cold and reused P95 is at least 20% of cold P95.

300 ms is the Engine-ready decision budget chosen for this experiment, not a historical baseline or a statement about user-perceived latency. The third criterion uses reused as an approximation of the theoretical ceiling prewarming could reach: if reused is equally slow, prewarming cannot fix the main bottleneck, and the model preparation, Skills materialization, Engine creation or history sync phase with the largest share in the report should be addressed first.

"Send to first output" in the report only counts the first non-empty `MessageDelta`; `ThinkingDelta` is not counted as visible text. Failed Engine acquisitions have no complete phase data and are excluded from the cold/warm distributions; the failure itself is still recorded by the existing turn terminal.

After collecting real-world data, keep a pre-optimization report for the same device and app version. Once an optimization is complete, resample with the same platform, sample size and command, and compare cold/reused P50 and P95, send-to-Engine-ready and send-to-first-output. Do not draw conclusions from a single best run.
