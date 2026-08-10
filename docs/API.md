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
| `/api/events` | `application/json` | Paginated request-level usage events |
| `/api/events/<event-id>` | `application/json` | One event with its raw source payload |
| `/api/prompts` | `application/json` | Paginated, searchable user prompts |
| `/api/prompts/<prompt-id>` | `application/json` | One normalized user prompt |

Unsupported API routes return a structured JSON `404`. Unsupported browser
routes return the plain-text body `not found`.

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

`total_tokens` uses the provider's counter semantics. OpenAI input already
includes cached input and output already includes reasoning. Anthropic cache
read/write input is additive while reasoning is an output breakdown. OpenCode,
Copilot, and Pi component counters are additive when their source omits a total.
An explicit provider total always wins.

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
including zero-usage days. It accepts the same `provider`, `window`, and
defaults as `/api/summary`.

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

## `GET /api/events`

Returns normalized usage events newest first. Pagination uses a compound
timestamp/event-ID cursor, so events with identical timestamps are neither
skipped nor repeated.

| Parameter | Required | Default | Meaning |
| --- | --- | --- | --- |
| `provider` | No | `codex` | Canonical provider name |
| `window` | No | `today` | `today`, `7d`, `30d`, or `all` |
| `limit` | No | `50` | Page size from 1 through 200 |
| `cursor` | No | none | URL-encoded `next_cursor` from the previous page |
| `model` | No | none | Exact model filter |
| `session` | No | none | Exact session-ID filter |
| `status` | No | none | Case-insensitive normalized source-status filter |

```bash
curl 'http://127.0.0.1:8787/api/events?provider=codex&window=7d&limit=25'
```

The response contains `events` and `next_cursor`. Submit the cursor unchanged
after URL encoding to fetch the next page. `next_cursor` is `null` when the
current result is not a full page.

Each event includes:

- `event_id`, `occurred_at`, provider, agent, session, model, client, and project;
- input, output, reasoning, cache-read, cache-write, and normalized total tokens;
- request, prompt, cost, AI-unit, credit, and code-change fields;
- `source_system`, `source_channel`, `source_request_id`, `source_locator`,
  `status`, and `duration_ms` when the provider records them;
- `total_source`, either `provider_reported` or `computed_provider_policy`.

Abbreviated response:

```json
{
  "events": [
    {
      "event_id": "5f…",
      "occurred_at": "2026-07-21T18:42:11Z",
      "provider_id": "codex",
      "agent_name": "codex",
      "session_id": "session-1",
      "model": "gpt-5",
      "input_tokens": 1200,
      "output_tokens": 180,
      "reasoning_tokens": 35,
      "cache_read_tokens": 700,
      "total_tokens": 1380,
      "requests": 1,
      "source_system": "codex",
      "source_channel": "jsonl",
      "source_request_id": "request-1",
      "status": "completed",
      "duration_ms": 842,
      "total_source": "provider_reported"
    }
  ],
  "next_cursor": "2026-07-21T18:42:11+00:00|5f…"
}
```

## `GET /api/events/<event-id>`

Returns one event and adds its preserved `raw_payload`. The canonical provider
must be supplied when the event is not from Codex.

```bash
curl 'http://127.0.0.1:8787/api/events/5f...?provider=codex'
```

The raw payload can contain prompts, paths, identifiers, or other sensitive
provider data. Do not expose this endpoint beyond the local machine.

## `GET /api/prompts`

Returns retrievable user prompts newest first. Prompts are normalized from
provider user-message records; assistant responses, tool results, and metadata
messages are excluded. Pagination uses the same stable timestamp/event-ID
cursor as `/api/events`.

| Parameter | Required | Default | Meaning |
| --- | --- | --- | --- |
| `provider` | No | `codex` | Canonical provider name |
| `window` | No | `today` | `today`, `7d`, `30d`, or `all` |
| `limit` | No | `50` | Page size from 1 through 200 |
| `cursor` | No | none | URL-encoded `next_cursor` from the previous page |
| `session` | No | none | Exact session-ID filter |
| `search` | No | none | Case-insensitive substring search over prompt text |

```bash
curl 'http://127.0.0.1:8787/api/prompts?provider=codex&window=30d&search=parser&limit=25'
```

Abbreviated response:

```json
{
  "prompts": [
    {
      "event_id": "7c…",
      "occurred_at": "2026-07-21T18:40:02Z",
      "provider_id": "codex",
      "agent_name": "codex",
      "session_id": "session-1",
      "model": "gpt-5",
      "project": "agentusage",
      "prompts": 1,
      "text": "Review the parser and add malformed-input tests.",
      "source_system": "codex",
      "source_channel": "jsonl",
      "source_locator": null
    }
  ],
  "next_cursor": "2026-07-21T18:40:02+00:00|7c…"
}
```

Prompt availability depends on the provider's local source data. Codex, Claude
Code, OpenCode, and Pi histories can contain prompt bodies. Current Copilot
usage sources expose counters and request metadata but not the original prompt
text, so its prompt list may be empty.

After upgrading an existing installation, run `au sync <provider>` once before
querying this endpoint so prompt records from earlier local history are indexed.
Synchronization is incremental and safe to repeat.

## `GET /api/prompts/<prompt-id>`

Returns one normalized prompt. Supply its canonical provider when it is not
Codex.

```bash
curl 'http://127.0.0.1:8787/api/prompts/7c...?provider=claude_code'
```

Prompt text can contain source code, credentials pasted into a conversation,
paths, or other sensitive data. Responses use `Cache-Control: no-store`, and
the server remains restricted to loopback hosts.

## Errors

API failures use an HTTP status and a stable JSON envelope:

```json
{
  "error": {
    "code": "invalid_limit",
    "message": "limit must be between 1 and 200"
  }
}
```

Validation errors return `400`, missing events/routes return `404`, unavailable
storage returns `503`, and unexpected query failures return `500`.

## Time and error behavior

- Window boundaries use the machine's local calendar and are serialized as UTC
  timestamps in summary responses.
- Event `occurred_at` values are source timestamps stored as UTC. Records with
  missing or invalid source timestamps are counted as malformed and skipped.
- Requests and prompts query normalized storage and do not scan provider source files.
- Unsupported providers/windows are rejected before storage is opened.
- The server has no built-in authentication or CORS headers.

Use `au server --verbose` to log request paths, provider windows, backend
selection, query durations, trend sizes, and background synchronization.
