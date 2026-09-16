/* Credentials remain in memory. All server values are rendered as text, never HTML. */
"use strict";
const $ = (id) => document.getElementById(id);
let token = "",
  identity = null,
  currentPolicy = null,
  visibleEvents = [],
  timer = null,
  refreshing = false,
  dirty = false;
const formatTime = (seconds) =>
  new Date(seconds * 1000).toLocaleString("en-GB");
function notice(message = "") {
  $("notice").textContent = message;
  $("notice").hidden = !message;
}
async function api(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    signal: AbortSignal.timeout(20000),
    headers: {
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
    },
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `Request failed (${response.status})`);
  }
  return response.status === 204 ? null : response.json();
}
function node(tag, text, className) {
  const el = document.createElement(tag);
  el.textContent = text;
  if (className) el.className = className;
  return el;
}
function setPolicy(policy) {
  currentPolicy = policy;
  $("revision").textContent = `Revision ${policy.revision}`;
  $("pause-state").textContent = policy.paused
    ? "Traffic is paused by policy."
    : "Policy is accepting evaluations.";
  $("pause-state").className = policy.paused ? "block" : "allow";
  for (const [id, key] of [
    ["paused", "paused"],
    ["ai-enabled", "ai_enabled"],
    ["require-ai", "require_ai"],
    ["block-uninspectable", "block_uninspectable"],
  ])
    $(id).checked = policy[key];
  for (const [id, key] of [
    ["alert-threshold", "alert_threshold"],
    ["block-threshold", "block_threshold"],
    ["max-body", "max_body_bytes"],
  ])
    $(id).value = policy[key];
  $("blocked-sources").value = policy.blocked_sources.join("\n");
}
function renderEvents(events) {
  visibleEvents = events;
  const tbody = $("events");
  tbody.replaceChildren();
  for (const incident of events) {
    const d = incident.decision,
      row = document.createElement("tr");
    row.append(node("td", formatTime(d.created_at)));
    const source = node("td", d.source);
    source.append(node("small", d.route), node("small", d.request_id));
    row.append(source);
    row.append(
      node("td", d.phase),
      node("td", `${d.action} · ${Math.round(d.score * 100)}%`, d.action),
    );
    const findings = node(
      "td",
      d.findings.map((f) => f.category.replaceAll("_", " ")).join(", ") ||
        "No findings",
    );
    findings.append(
      node("small", `AI: ${d.ai_status} · policy ${d.policy_revision}`),
    );
    row.append(findings);
    const review = document.createElement("td");
    if (incident.acknowledged_by)
      review.textContent = `Reviewed by ${incident.acknowledged_by}`;
    else if (d.action !== "allow" && identity.role === "operator") {
      const button = node("button", "Acknowledge");
      button.type = "button";
      button.addEventListener("click", async () => {
        button.disabled = true;
        try {
          await api(`/v1/events/${d.id}/acknowledge`, { method: "POST" });
          await refresh();
        } catch (e) {
          notice(e.message);
        } finally {
          button.disabled = false;
        }
      });
      review.append(button);
    } else review.textContent = d.action === "allow" ? "—" : "Awaiting review";
    row.append(review);
    tbody.append(row);
  }
  if (!events.length) {
    const row = document.createElement("tr"),
      cell = node("td", "No decisions match this view.");
    cell.colSpan = 6;
    row.append(cell);
    tbody.append(row);
  }
}
async function refresh() {
  if (!token || refreshing) return;
  refreshing = true;
  try {
    const query = new URLSearchParams({
      limit: "100",
      source: $("source-filter").value.trim(),
      incidents: String($("incidents-only").checked),
    });
    const [summary, policy, events, audit, gateway] = await Promise.all([
      api("/v1/summary"),
      api("/v1/policy"),
      api(`/v1/events?${query}`),
      api("/v1/audit"),
      api("/v1/gateway"),
    ]);
    for (const action of ["allow", "alert", "block"])
      $(`${action}-count`).textContent =
        summary.decisions[action].toLocaleString("en-GB");
    $("review-count").textContent = summary.unacknowledged;
    $("pending-alerts").textContent = summary.pending_alerts;
    $("failed-alerts").textContent = summary.failed_alerts;
    if (!dirty) setPolicy(policy);
    $("gateway-mode").textContent = gateway.available
      ? {
          enforce: "Enforcing",
          monitor: "Monitoring only",
          disabled: "Disabled",
        }[gateway.mode]
      : "Unreachable";
    $("gateway-failure").textContent = gateway.available
      ? gateway.mode === "enforce" && !gateway.fail_open
        ? "Blocks traffic"
        : "Allows traffic"
      : "Unknown";
    if (policy.paused) {
      $("pause-state").textContent = !gateway.available
        ? "Pause enabled; Gail status is unavailable."
        : gateway.mode === "enforce"
          ? "Governed traffic is paused."
          : "Pause enabled in policy; Gail is not enforcing it.";
    }
    renderEvents(events);
    $("audit").replaceChildren(
      ...audit.map((a) => {
        const li = node(
          "li",
          `${a.actor} · ${a.operation.replaceAll("_", " ")} · ${a.target}`,
        );
        li.prepend(node("time", formatTime(a.created_at)));
        return li;
      }),
    );
    if (!audit.length)
      $("audit").append(node("li", "No management changes recorded."));
    const total = Object.values(summary.decisions).reduce((a, b) => a + b, 0);
    $("decision-mix").replaceChildren(
      ...["allow", "alert", "block"].map((action) =>
        node(
          "span",
          `${action} ${total ? Math.round((summary.decisions[action] / total) * 100) : 0}%`,
          action,
        ),
      ),
    );
    $("mix-description").textContent =
      `${total.toLocaleString("en-GB")} recorded evaluations`;
    $("updated").textContent =
      `Updated ${new Date().toLocaleTimeString("en-GB")}`;
    $("connection").textContent = "Connected";
    $("connection").className = "status live";
  } catch (e) {
    $("connection").textContent = "Data unavailable";
    $("connection").className = "status stale";
    notice(e.message);
  } finally {
    refreshing = false;
  }
}
$("login").addEventListener("submit", async (event) => {
  event.preventDefault();
  notice();
  token = $("token").value.trim();
  $("token").value = "";
  try {
    identity = await api("/v1/session");
    if (!["viewer", "operator"].includes(identity.role))
      throw new Error("Use a viewer or operator token for the dashboard.");
    const products = await api("/v1/products");
    $("products").replaceChildren(
      ...products.map((p) => {
        const link = node("a", p.name);
        const url = new URL(p.url);
        if (["http:", "https:"].includes(url.protocol)) link.href = url.href;
        return link;
      }),
    );
    $("identity").textContent = `${identity.principal} · ${identity.role}`;
    $("policy-fields").disabled = identity.role !== "operator";
    $("workspace").hidden = false;
    $("login").hidden = true;
    await refresh();
    clearInterval(timer);
    timer = setInterval(() => {
      if (!document.hidden) refresh();
    }, 10000);
  } catch (e) {
    token = "";
    notice(e.message);
  }
});
$("disconnect").addEventListener("click", () => {
  token = "";
  identity = null;
  clearInterval(timer);
  $("workspace").hidden = true;
  $("login").hidden = false;
  $("connection").textContent = "Disconnected";
  $("connection").className = "status";
  dirty = false;
  notice();
});
$("policy-form").addEventListener("input", () => {
  dirty = true;
});
$("policy-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!currentPolicy) return;
  notice();
  const policy = {
    ...currentPolicy,
    paused: $("paused").checked,
    ai_enabled: $("ai-enabled").checked,
    require_ai: $("require-ai").checked,
    block_uninspectable: $("block-uninspectable").checked,
    alert_threshold: Number($("alert-threshold").value),
    block_threshold: Number($("block-threshold").value),
    max_body_bytes: Number($("max-body").value),
    blocked_sources: $("blocked-sources")
      .value.split("\n")
      .map((v) => v.trim())
      .filter(Boolean),
  };
  try {
    setPolicy(
      await api("/v1/policy", { method: "PUT", body: JSON.stringify(policy) }),
    );
    dirty = false;
    await refresh();
  } catch (e) {
    notice(e.message);
  }
});
$("refresh").addEventListener("click", () => {
  dirty = false;
  notice();
  refresh();
});
$("source-filter").addEventListener("change", refresh);
$("incidents-only").addEventListener("change", refresh);
$("export").addEventListener("click", () => {
  const url = URL.createObjectURL(
      new Blob([JSON.stringify(visibleEvents, null, 2)], {
        type: "application/json",
      }),
    ),
    link = document.createElement("a");
  link.href = url;
  link.download = "aria-decisions.json";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
});
