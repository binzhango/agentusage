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
| `/` | `text/html; charset=utf-8` | Usage timeline across visible providers |
| `/provider/<name>` | `text/html; charset=utf-8` | Model timeline, usage breakdown, and prompt history |
| `/api/providers` | `application/json` | Provider names and storage availability |
| `/api/summary` | `application/json` | Aggregate usage for one provider and window |
| `/api/trend` | `application/json` | Daily or hourly totals by model |
| `/api/events` | `application/json` | Paginated normalized usage events |
| `/api/events/<event-id>` | `application/json` | One event with its raw source payload |
| `/api/prompts` | `application/json` | Paginated, searchable user prompts |
| `/api/prompts/<prompt-id>` | `application/json` | One normalized user prompt |

Both browser pages accept the `window` parameter. For example, `/provider/codex?window=30d` opens the thirty-day Codex view. The HTML loads JavaScript and CSS from the binary’s embedded `/assets/` routes. Unsupported API routes return a structured JSON `404`.

## `GET /api/providers`

Returns every provider known to the server. `available` is `true` when
Agentusage can open initialized SQLite or PostgreSQL storage for the provider.
An available provider can still have zero usage in a selected period.

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
curl 'http://127.0.0.1:8787/api/summary?provider=codex&window=7d'
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

The Codex dashboard calculates **Cached input** as `100 × cache_read_tokens / input_tokens` over the window. It shows a dash for zero input. Both the summary and the model buckets retain `cache_write_tokens` in JSON, including when the UI hides a zero metric or column. Missing source cache counters normalize to zero, so JSON alone cannot distinguish an omitted counter from an explicit zero.

Abbreviated seven-day Codex response from sample data (one model shown):

```json
{
  "from": "2026-11-17T05:00:00Z",
  "to": "2026-11-24T05:00:00Z",
  "sessions": 20,
  "requests": 125,
  "prompts": 6,
  "input_tokens": 1969032,
  "output_tokens": 10000,
  "reasoning_tokens": 0,
  "cache_read_tokens": 599421,
  "cache_write_tokens": 0,
  "total_tokens": 1979032,
  "cost_usd": 2.846008,
  "models": {
    "gpt-5.4-mini": {
      "requests": 46,
      "input_tokens": 727116,
      "output_tokens": 3680,
      "cache_read_tokens": 223619,
      "cache_write_tokens": 0,
      "total_tokens": 730796
    }
  }
}
```

The real response includes all scalar fields and empty objects when a dimension
has no data.

## `GET /api/trend`

Returns time buckets in chronological order. Use the same `provider` and `window` values as `/api/summary`.

| `interval` | Allowed windows | Buckets returned |
| --- | --- | --- |
| `day` (default) | `today`, `7d`, `30d`, `all` | One per local calendar day, including days with zero usage. The `all` chart covers the latest 90 days. |
| `hour` | `today` | One per elapsed hour from local midnight, including the hour in progress and hours with zero usage. |

```bash
curl 'http://127.0.0.1:8787/api/trend?provider=codex&window=today&interval=hour'
curl 'http://127.0.0.1:8787/api/trend?provider=codex&window=7d'
```

| Field | Type | Meaning |
| --- | --- | --- |
| `bucket_start` | RFC 3339 timestamp | Start of the interval in UTC |
| `date` | `YYYY-MM-DD` string | Local calendar date |
| `total_tokens` | integer | Total tokens for the interval |
| `input_tokens` | integer | Input tokens for the interval |
| `output_tokens` | integer | Output tokens for the interval |
| `cache_read_tokens` | integer | Cache-read tokens for the interval |
| `models` | object | Total tokens keyed by model |

First daily bucket from the seven-day sample, shown as a one-element excerpt:

```json
[
  {
    "bucket_start": "2026-11-17T05:00:00Z",
    "date": "2026-11-17",
    "input_tokens": 470840,
    "output_tokens": 2160,
    "cache_read_tokens": 138797,
    "total_tokens": 473000,
    "models": {
      "gpt-5.4-mini": 212388,
      "gpt-5.5": 142824,
      "gpt-6.2-sol": 117788
    }
  }
]
```

Use `bucket_start` to align provider rows and distinguish hours that share a local date. Summing the `all` trend gives the latest 90 days. Use `/api/summary?provider=codex&window=all` for lifetime totals. An unsupported interval or `interval=hour` with a longer window returns HTTP `400` with error code `invalid_interval`.

## `GET /api/events`

Returns normalized usage events newest first, including rows with `requests = 0` such as prompt events. The TUI’s recent-request list selects rows with a positive request count. Pagination orders by timestamp and event ID so events sharing a timestamp have a stable order.

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
# Set event_id to an event_id from the list response.
curl "http://127.0.0.1:8787/api/events/${event_id}?provider=codex"
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

The browser requests three prompts per page from the provider list and ten from a provider page. Both views support search and pagination. Direct API clients can choose their own page size.

## `GET /api/prompts/<prompt-id>`

Returns one normalized prompt. Supply its canonical provider when it is not
Codex.

```bash
# Set prompt_id to an event_id from the Claude Code prompt list.
curl "http://127.0.0.1:8787/api/prompts/${prompt_id}?provider=claude_code"
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
