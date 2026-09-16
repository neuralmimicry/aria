# HTTP contract, version 1

Aria returns JSON and requires `Authorization: Bearer <token>` for all `/v1/*` endpoints and `/metrics`. `/`, dashboard assets, `/healthz` and `/readyz` are public and contain no audit data. API credentials are not accepted through query strings, cookies or forwarded identity headers.

| Endpoint | Capability | Behaviour |
|---|---|---|
| `POST /v1/evaluate` | Evaluator | Assess and persist one request or response |
| `GET /v1/session` | Any authenticated role | Return principal, role and service version |
| `GET /v1/summary` | Viewer / operator | Counts, pending deliveries and unreviewed incidents |
| `GET /v1/gateway` | Viewer / operator | Actual Gail mode and outage behaviour; reports unavailable without hiding persisted audit data |
| `GET /v1/events` | Viewer / operator | Recent decisions and acknowledgements |
| `POST /v1/events/{id}/acknowledge` | Operator | Attribute an acknowledgement; repeated or invalid acknowledgement returns 409 |
| `GET /v1/policy` | Viewer / operator | Read the current policy |
| `PUT /v1/policy` | Operator | Replace policy using its current revision; stale revision returns 409 |
| `GET /v1/policy/history` | Viewer / operator | Latest 100 policy revisions with actor and time |
| `GET /v1/audit` | Viewer / operator | Latest 100 management operations |
| `GET /v1/products` | Viewer / operator | Configured product navigation |
| `GET /metrics` | Viewer / operator | Prometheus metrics without client or content labels |

Evaluation input:

```json
{
  "request_id": "110496c9-3b0b-4ab7-857c-6b1bd70a758a",
  "phase": "request",
  "source": "refiner",
  "route": "/v1/llm/complete",
  "content": "Explain this public dataset.",
  "body_bytes": 124,
  "inspection_complete": true
}
```

`phase` is `request` or `response`. Gail supplies a fresh UUID for each exchange and derives `source` from its authenticated client configuration. `content` is semantic text extracted from the body; transport credentials are excluded from requests. Credential-like text inside prompts and response fields remains inspectable. `inspection_complete` is false for opaque media or unsupported encodings. The evaluator credential is a trusted service capability and must never be distributed to end users.

A decision contains `protocol_version: 1`, a decision `id`, the exchange `request_id`, `phase`, `source`, `route`, UTC epoch `created_at`, `policy_revision`, `action`, `score`, `findings`, `ai_status`, model/provider provenance, `content_sha256`, `body_bytes` and `evaluation_ms`. Actions are `allow`, `alert` and `block`. Findings use fixed category and explanation codes; they contain no model-generated prose.

`control_revision` records the latest pause/source-block policy checked before release. It may differ from `policy_revision` if an operator intervenes during inference; both snapshots remain available in policy history. Older records may omit this field.

Identical `(source, request_id, phase)` retries return the original persisted decision. Reusing that key with different input returns 409. An audit-store failure returns 503 instead of an unrecorded successful decision. Capacity exhaustion returns 429. Generate a new exchange UUID to assess an input again under a newer policy.

`GET /v1/events` accepts `limit` (1–200, default 50), `source` (exact client ID), `incidents=true`, and `before` (exclusive UTC epoch seconds). The dashboard displays the latest 100 records. Export contains exactly the visible records. The `before` filter is a time filter, not a cursor for exhaustively paging records sharing the same second.

Policy updates require every field from `GET /v1/policy`. The database atomically checks `revision`, writes revision + 1, records the actor and appends history. Scores use the maximum finding score; AI cannot reduce a deterministic score. `paused`, a blocked source, required-AI failure and exceeded body limits produce score 1. Thresholds satisfy `0 <= alert_threshold < block_threshold <= 1`. An acknowledgement records review only and never releases a blocked exchange.

Gail exposes:

- `POST /v1/internal/aria/assess`, authorised only by its dedicated assessment token. Input is `{content, phase}`. Gail fixes the system instructions, provider selection constraints, source, budget and allowed output schema. Unknown input fields and invalid model output are rejected. Aria receives `{score, categories, provider, model}`.
- `GET /v1/status/governance`, authorised by an ordinary Gail `status` capability. It reports `mode`, `fail_open`, observed evaluations, blocks and transport failures without credentials.
- `GET /v1/internal/aria/status` provides the same read-only status using Aria’s dedicated assessment credential, for dashboard monitoring.

Successful governed HTTP responses include `X-Aria-Request-Id` and `X-Aria-Status` (`checked`, `would_block`, `unavailable`). A blocked exchange receives HTTP 403 with a `governance_error` and its Aria decision ID. Unavailable governance in fail-closed enforcement receives 503. Capacity and buffering limits also apply in monitoring mode.
