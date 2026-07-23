# HTTP API reference

Agentusage includes a local read-only JSON API used by its browser dashboard
and available to scripts or custom integrations.

Start the server:

```bash
au server
```

The default base URL is `http://127.0.0.1:8787`.

## Routes

All supported routes use `GET`.

| Endpoint | Content type | Purpose |
| --- | --- | --- |
| `/` | `text/html; charset=utf-8` | Main browser dashboard |
| `/provider/<name>` | `text/html; charset=utf-8` | Full-page provider dashboard |
| `/api/providers` | `application/json` | Provider names and storage availability |
| `/api/summary` | `application/json` | Aggregate usage for one provider and window |
| `/api/trend` | `application/json` | Daily totals and per-model trend data |

Unsupported routes or methods return `404` with the plain-text body
`not found`.

## `GET /api/providers`

Returns every provider known to the server. `available` is `true` when
Agentusage can open initialized SQLite or PostgreSQL storage for the provider.
It does not mean the selected time window contains events.

```bash
curl 'http://127.0.0.1:8787/api/providers'
```

```json
[
  { "name": "codex", "available": true },
  { "name": "claude_code", "available": true },
  { "name": "opencode", "available": false },
  { "name": "copilot", "available": true },
  { "name": "pi", "available": true }
]
```

This endpoint has no query parameters.

## `GET /api/summary`

Returns aggregate usage for one provider and time window.

| Parameter | Required | Default | Accepted values |
| --- | --- | --- | --- |
| `provider` | No | `codex` | `codex`, `claude_code`, `opencode`, `copilot`, `pi` |
| `window` | No | `today` | `today`, `7d`, `30d`, `all` |

Compatibility aliases:

- `claude` for `claude_code`;
- `open_code` for `opencode`;
- `7days` for `7d`;
- `30days` for `30d`;
- `all_time` for `all`.

```bash
curl 'http://127.0.0.1:8787/api/summary?provider=codex&window=30d'
```

### Response fields

| Field | Type | Meaning |
| --- | --- | --- |
| `from`, `to` | RFC 3339 timestamp | Inclusive start and exclusive end |
| `sessions` | integer | Distinct sessions |
| `requests`, `prompts` | integer | Request and prompt counts |
| `input_tokens`, `output_tokens` | integer | Input and generated tokens |
| `reasoning_tokens` | integer | Provider-reported reasoning tokens |
| `cache_read_tokens`, `cache_write_tokens` | integer | Prompt-cache activity |
| `total_tokens` | integer | Total normalized token volume |
| `cost_usd` | number | Estimated or provider-reported USD cost |
| `ai_units_nano` | integer | Provider-native AI units in nano-units |
| `request_multiplier` | number | Sum of provider request multipliers |
| `ai_credits` | number | Provider-reported AI credits |
| `lines_added`, `lines_removed` | integer | Imported code-change counts |
| `models` | object | Usage buckets keyed by model |
| `providers` | object | Usage buckets keyed by upstream provider |
| `clients` | object | Usage buckets keyed by client |
| `projects` | object | Usage buckets keyed by project or workspace |
| `tools`, `languages` | object | Event counts keyed by tool or language |
| `primary_used_percent` | number or `null` | Latest known primary quota usage |
| `primary_window_minutes` | integer or `null` | Primary quota-window duration |
| `primary_resets_at` | integer or `null` | Provider reset timestamp |

Entries in usage bucket objects contain request, token, cost, AI-unit,
multiplier, and credit fields where available.

Abbreviated response:

```json
{
  "from": "2026-06-22T04:00:00Z",
  "to": "2026-07-22T04:00:00Z",
  "sessions": 18,
  "requests": 246,
  "prompts": 91,
  "input_tokens": 315000,
  "output_tokens": 42000,
  "reasoning_tokens": 12000,
  "cache_read_tokens": 98000,
  "cache_write_tokens": 7000,
  "total_tokens": 474000,
  "cost_usd": 3.82,
  "models": {
    "gpt-5": {
      "requests": 180,
      "input_tokens": 250000,
      "output_tokens": 35000,
      "total_tokens": 394000,
      "cost_usd": 3.21
    }
  }
}
```

The real response includes all scalar fields and empty objects when a dimension
has no data.

## `GET /api/trend`

Returns one point for every local calendar day in the selected window,
including zero-usage days. It accepts the same `provider`, `window`, defaults,
and aliases as `/api/summary`.

```bash
curl 'http://127.0.0.1:8787/api/trend?provider=codex&window=30d'
```

| Field | Type | Meaning |
| --- | --- | --- |
| `date` | `YYYY-MM-DD` string | Local calendar date |
| `total_tokens` | integer | Total tokens for the day |
| `input_tokens` | integer | Input tokens for the day |
| `output_tokens` | integer | Output tokens for the day |
| `cache_read_tokens` | integer | Cache-read tokens for the day |
| `models` | object | Daily total tokens keyed by model |

```json
[
  {
    "date": "2026-07-21",
    "total_tokens": 48210,
    "input_tokens": 31140,
    "output_tokens": 7070,
    "cache_read_tokens": 10000,
    "models": {
      "gpt-5": 36100,
      "gpt-5-mini": 12110
    }
  }
]
```

For `today`, `7d`, and `30d`, the trend covers the summary period. For `all`,
the summary starts at 1970-01-01 while the trend is limited to the latest 90
days.

## Time and error behavior

- Window boundaries use the machine's local calendar and are serialized as UTC
  timestamps in summary responses.
- Requests query normalized storage and do not scan provider source files.
- Unsupported windows and unavailable backends are logged by the server.
- Errors do not currently use a structured JSON error schema.
- The server has no built-in authentication or CORS headers.

Use `au server --verbose` to log request paths, provider windows, backend
selection, query durations, trend sizes, and background synchronization.
