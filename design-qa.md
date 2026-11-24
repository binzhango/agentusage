# Browser dashboard verification

The [overview capture](docs/images/browser-dashboard.jpg) uses synthetic usage. The peak guide and the colored provider rows share the same time axis. The layout was checked at 1376 and 390 pixels, including dark mode and alignment of the provider list on mobile.

Browser checks covered provider selection, keyboard inspection, date ranges, hiding and restoring providers, and prompt search, expansion, and pagination. Opening the provider page changes the chart rows to models.

The [seven-day sample export](docs/images/svg-export.svg) shows 30.4% cached input across 125 Codex requests. Its model table contains exact cache reads. All model writes in this sample are zero, so the write column is hidden. The [release smoke tests](docs/RELEASING.md#release-smoke-tests) also cover positive writes and periods without input.

final result: passed
