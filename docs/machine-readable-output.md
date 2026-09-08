# Machine-readable output

Pass `--json` to one-shot commands. texe writes one closed JSON object to
standard output and progress to standard error. `texe watch --json` writes one
`texe.watch-event/v1` object per line.

Every texe-owned protocol starts at v1. A schema version changes only when a
consumer must handle an incompatible shape; it is independent of the texe
crate version and of third-party tool versions.

| Result | Schema identifier | JSON Schema |
| --- | --- | --- |
| Project manifest | `texe.project/v1` | [`texe.project.schema.json`](../schemas/texe.project.schema.json) |
| Project lock | `texe.lock/v1` | [`texe.lock.schema.json`](../schemas/texe.lock.schema.json) |
| Build | `texe.build-report/v1` | [`texe.build-report.schema.json`](../schemas/texe.build-report.schema.json) |
| Build progress (stderr) | `texe.build-progress/v1` | [`texe.build-progress.schema.json`](../schemas/texe.build-progress.schema.json) |
| Clean and clean dry run | `texe.clean-report/v1`, `texe.clean-dry-run/v1` | [`texe.clean.schema.json`](../schemas/texe.clean.schema.json) |
| Storage | `texe.storage-report/v1` | [`texe.storage-report.schema.json`](../schemas/texe.storage-report.schema.json) |
| Initialization | `texe.init-report/v1` | [`texe.init-report.schema.json`](../schemas/texe.init-report.schema.json) |
| Bare invocation | `texe.bare-report/v1` | [`texe.bare-report.schema.json`](../schemas/texe.bare-report.schema.json) |
| Doctor | `texe.doctor-report/v1` | [`texe.doctor-report.schema.json`](../schemas/texe.doctor-report.schema.json) |
| Editor integration | `texe.editor-report/v1` | [`texe.editor-report.schema.json`](../schemas/texe.editor-report.schema.json) |
| Editor paths (read-only) | `texe.editor-context/v1` | [`texe.editor-context.schema.json`](../schemas/texe.editor-context.schema.json) |
| Editor settings preview | `texe.editor-preview/v1` | [`texe.editor-preview.schema.json`](../schemas/texe.editor-preview.schema.json) |
| Existing paper preflight | `texe.adoption-report/v1` | [`texe.adoption-report.schema.json`](../schemas/texe.adoption-report.schema.json) |
| Error | `texe.error/v1` | [`texe.error.schema.json`](../schemas/texe.error.schema.json) |
| Watch event | `texe.watch-event/v1` | [`texe.watch-event.schema.json`](../schemas/texe.watch-event.schema.json) |
| Local viewer status | `texe.viewer-status/v1` | [`texe.viewer-status.schema.json`](../schemas/texe.viewer-status.schema.json) |

Consumers should select behavior from the `schema` field, reject unsupported
versions, and ignore presentation written to standard error. Golden v1
artifacts in `tests/golden/v1` are validated against these schemas in CI.

During JSON builds, stderr also carries newline-delimited `texe.build-progress/v1`
objects with `phase` and `elapsed_millis`. Consumers may use these for live UI;
ignore other stderr lines and unknown progress schemas. `--quiet` suppresses
progress events. Stdout remains a single final result or error object.

The build report's `duration_millis` measures the full build operation, including
project loading, toolchain/package preparation, and publication. Cached builds
report the time spent validating reuse rather than always reporting zero.

The local viewer status includes `state` (`waiting`, `building`, `failed`, or
`ready`) alongside the successful-PDF generation. Failure does not advance the
generation. Clients should treat a failed status request as disconnected and
keep the displayed PDF marked as potentially stale.
