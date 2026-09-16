# Operating Aria

## Boundaries and failure behaviour

Gail’s HTTP checks cover native and compatible chat/response APIs, direct completions, transcription routes, neuromorphic analysis/prediction, AER conversion and explicit trading evaluation. In-process calls to `GailService::complete` and `direct_complete` are also checked. HTTP-scoped and classifier-scoped task-local markers prevent duplicate or recursive assessment; neither can be set by request headers, model output or caller-supplied source fields.

Aria does not control independent provider access outside Gail, arbitrary trading configuration operations or non-AI trading execution. A global pause stops governed evaluations at their next boundary; it does not cancel an already-running provider operation or reverse external side effects. Inference results are rechecked for a pause or source block activated during assessment. Ordinary policy settings use the snapshot captured at the start of that evaluation.

Response inspection delays streaming until the complete bounded response is checked. The current implementation provides no incremental release of an unapproved token. Bodies that exceed Gail’s buffer limit are rejected even in monitoring mode; larger media requires a higher limit and an explicit policy choice about uninspectable content. Aria does not download arbitrary URLs, decode media, execute tools or run submitted code to analyse it.

Gail independently caps AER dense spike allocations at 16 MiB, including lengths inferred from sparse addresses. This prevents small binary inputs from allocating enormous output buffers, regardless of model judgement or governance mode.

Use `governance.mode: monitor` to collect decisions without applying content blocks. `enforce` applies blocks; `disabled` preserves Gail’s existing behaviour. In enforcement, transport errors fail closed unless `fail_open` is explicitly enabled. Aria independently treats unavailable, malformed or timed-out AI as a block when `require_ai` is true. Optional AI failure raises an alert. Monitor mode observes pauses and blocks without stopping application traffic.

Gail’s classifier uses existing configured providers with a single selected candidate and a dedicated four-request admission limit. Each classifier call is bounded by its assessment deadline; Aria’s deadline should be longer, and Gail’s outer evaluation deadline longer again. Defaults are 12, 15 and 20 seconds respectively. Size concurrency for the available provider capacity: every permitted exchange can require two additional model calls. No automatic retry of the original AI operation is introduced.

The assessment path excludes classifier prompts from Gail’s content ledger and AARNN learning mirrors. Ordinary Gail logging, comparison and mirroring retain their existing behaviour. Aria’s external response block does not undo internal work that Gail performed while producing that response. Governed services should also enforce their own authorisation and execution limits.

## Persistence and scale

SQLite uses one asynchronous SQL connection and one service replica. PostgreSQL uses a bounded pool and supports shared policy, event and alert state across replicas. Schema setup is idempotent and serialised with a PostgreSQL transaction advisory lock. Policy writes use compare-and-swap revisions; concurrent writers cannot silently overwrite each other.

Decision records store content hashes, stable findings and model provenance. They do not retain raw prompts, responses or classifier reasoning. Hashes support correlation, not reconstruction or cryptographic proof that the database has not been altered. Exact AI replay is not claimed. Keep access-controlled upstream context separately if an investigation requires it, and apply an appropriate retention policy.

Database backups must include policies, policy history, decisions, acknowledgements, management audit and the alert outbox. No automated retention deletes evidence. Monitor database growth; for large installations, archive reviewed records through an approved database maintenance process and account for the resulting changes to lifetime counts. Metrics summarise persisted rows and may become more expensive as retention grows.

Readiness tests the database, not Gail. This avoids circular startup and lets the dashboard remain available during a model outage. Inspect `ai_status`, pending/failed alerts, Gail’s governance status and Prometheus metrics for degraded operation. Do not use `/readyz` as evidence that a model is available.

## Alert receivers

An alert is enqueued in the same database transaction as its decision whenever a webhook is configured. Workers claim short leases, deliver up to four alerts concurrently and make at most eight attempts with exponential delays and jitter. Exhausted deliveries remain visible in `failed_alerts` and the database. Acknowledge incidents in the dashboard; investigate and deliberately reschedule exhausted outbox rows through your database operations process if delivery is still required.

Each request carries `X-Aria-Event-Id`, `X-Aria-Timestamp` and `X-Aria-Signature: sha256=<hex>`. Verify HMAC-SHA256 with `ARIA_WEBHOOK_SECRET` over:

```text
<timestamp>.<exact request body bytes>
```

Reject stale timestamps and compare signatures in constant time. Deduplicate by event ID: a receiver can observe a repeated delivery if a worker dies after successful receipt but before recording success. Respond with a successful HTTP status only after accepting the event durably. The payload is the redacted decision record. Redirects are not followed.

## Product and deployment integration

The Customers catalogue advertises Aria with no public access; Conductor links to its dashboard. These changes do not turn product catalogue grants into Aria API permissions. Viewer and operator access remains explicit through Aria credentials. Existing ingress authorisation may be configured using `continuum_tenant_aria_ingress_annotations`, in addition to the API credential requirement.

The combined Ansible playbook shares credentials between Aria and Gail and mounts the Prometheus viewer token through a Kubernetes Secret. Tokens never appear in ConfigMaps or the frontend. Configure individual operator entries with `continuum_tenant_aria_operator_credentials`. Rotate corresponding service credentials together, account for brief fail-closed interruptions during rollout, and keep the private controller secret directory backed up or use explicit vault-managed values.

The supplied role consumes a published, versioned image. The application Containerfile builds the Rust service and embeds dashboard assets; it runs without root and the deployment disallows privilege escalation and filesystem writes outside data and temporary mounts. For PostgreSQL replicas, distribute pods through your cluster placement policy and size the database and model-serving pools together.
