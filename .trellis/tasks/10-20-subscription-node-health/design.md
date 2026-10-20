# Design: subscription and node settings, automated health verification

## 1. Settings model

Add nested structs to `config/verge.rs` with field-level patch merge:

```rust
pub struct SubscriptionSettings {
    pub default_update_interval: Option<u64>,
    pub update_on_startup: Option<bool>,
    pub update_only_current: Option<bool>,
    pub notify_on_failure: Option<bool>,
}

pub struct ProbeTarget {
    pub url: String,
    pub expected: String,
    pub keyword: Option<String>,
    pub timeout_ms: Option<u32>,
}

pub struct NodeHealthSettings {
    pub enabled: Option<bool>,
    pub interval_minutes: Option<u64>,
    pub deep_interval_minutes: Option<u64>,
    pub scope: Option<String>,
    pub l1_targets: Option<Vec<ProbeTarget>>,
    pub l2_targets: Option<Vec<ProbeTarget>>,
    pub verify_on_select: Option<bool>,
    pub auto_failover: Option<bool>,
}

pub struct NodeDisplaySettings {
    pub hide_timeout: Option<bool>,
    pub hide_untested: Option<bool>,
    pub hide_unhealthy: Option<bool>,
    pub hide_slow_ms: Option<u16>,
    pub default_sort: Option<u8>,
}
```

Integration points:

- `IVerge`: four optional fields.
- `template()`: defaults.
- `patch_config`: merge nested `Some` fields.
- `verge_registry!`: `NoEffect`; executor refreshes managers.
- `feat/executor.rs`: refresh `NodeHealthManager` after a successful Verge patch.
- `pnpm gen:contract` and `pnpm i18n:types`.

Defaults:

| Field | Default |
|---|---|
| health enabled | false |
| L1 interval | 10 min |
| L2 interval | 60 min |
| scope | current_group |
| L1 targets | `https://cp.cloudflare.com/generate_204` expected 204 |
| L2 targets | Google robots 200, GitHub favicon 200, Cloudflare trace 200 |
| verify on select | true |
| auto-failover | false |
| hide timeout/untested/unhealthy | false |
| hide slow | 0 (off) |

## 2. Expected-status probes

mihomo supports `expected` on the delay endpoint:

```text
GET /proxies/{name}/delay?url=...&timeout=...&expected=204
GET /providers/proxies/{provider}/{node}/healthcheck?url=...&timeout=...&expected=200
```

The current plugin sends only `url` and `timeout`. Add a local Rust command (or extend the plugin) that passes `expected` and returns:

```rust
struct ProbeResult {
    delay_ms: u32,
    ok: bool,
    error: Option<String>,
}
```

`expected` examples: `204`, `200`, `200-299`, `200/204`.

## 3. `NodeHealthManager`

New `core/node_health.rs`, singleton, initialized in `resolve_setup`.

Cycle:

1. Wait for core/profile resolution and a startup delay.
2. Snapshot profile `sync_id`, scope, groups, and nodes; record a generation counter.
3. Run L1 with bounded concurrency (max 10); optional L2 with max 5.
4. Per request timeout: 5 s (L1), 8 s (L2); cycle timeout: 60 s.
5. If generation changed, cancel and discard.
6. Compute state and update the health map.
7. Emit a health snapshot event; persist at most every 5 minutes.

State computation:

- `unusable`: all L1 targets fail.
- `site_blocked`: L1 passes and at least one L2 target fails; store failed target names.
- `degraded`: L1/L2 pass but jitter > threshold, success rate < 80%, or latency > threshold.
- `healthy`: otherwise.
- `unverified`: no data or older than 2 intervals.
- `testing`: in progress.

Cache:

- `HashMap<HealthKey, NodeHealth>` keyed by profile `sync_id` + group + node.
- Max 5,000 entries; prune keys not in the current proxy view each cycle.
- Persist to `node-health.json`, max 2 MB, atomic write, throttled.
- On parse failure, start empty and log.

Cancellation:

- `watch` channel for settings changes.
- Generation counter and `CancellationToken`.
- App exit signals cancellation and waits up to 2 s.
- Single-flight per node/group; manual tests and background tests share a running set.

Backoff:

- Consecutive failures multiply the next check interval by 2, capped at 4x.
- A success resets the multiplier.

## 4. L3 content probe

New `core/probe.rs`:

- Uses `reqwest` with the local mixed port as proxy.
- Options: URL, timeout, expected status, keyword, max body bytes (64 KB), follow redirects (bounded).
- Returns status, TTFB, total time, body size, keyword match.
- Used for selected-node verification and manual deep tests.
- If the selected node changes during the probe, discard the result.
- Does not run for all nodes in the background.

## 5. Frontend health state

- New `services/node-health.ts` or extension of `delayManager`.
- Backend emits a snapshot; frontend stores a versioned map, like `DelayManager` group snapshots.
- Group-level listeners notify with `requestAnimationFrame`; no per-node subscriptions.
- Node cards display badges and a tooltip with L1/L2 details.
- Sorting uses health rank first, then latency.
- Filtering uses health state + `classifyDelay`.

## 6. Node display filter

- `HeadState` adds `hideUnavailable?: boolean`.
- Global defaults come from `NodeDisplaySettings`; undefined per-group value follows the global default.
- `ProxyGroupTools` adds a `FilterAltRounded`/`FilterAltOffRounded` icon.
- `filterSort` receives display options:
  - hide timeout/error
  - hide untested
  - hide unhealthy
  - hide slow above threshold
  - pinned current node
- Search bypasses hide filters.
- Hidden count returned to the group header.
- Manual "test all" uses all group members, not only visible ones.

## 7. Legacy migration

- If `enable_auto_delay_detection = true` and `node_health.enabled` is unset:
  - enabled = true
  - scope = current_node
  - interval = `auto_delay_detection_interval_minutes` or 5
- Hide the old Misc rows; keep fields for one release.
- Move `default_latency_test`/`default_latency_timeout` controls into the new section; keep the keys.

## 8. Error handling

| Failure | Behavior |
|---|---|
| Core not running | Skip cycle; keep last state |
| Profile/group set changes | Invalidate generation; discard results |
| Provider node | Use provider healthcheck endpoint |
| Invalid target URL | Fall back to default HTTPS 204 |
| All targets fail | Mark unusable; back off |
| Request timeout | Record failure; continue other nodes |
| App exit | Cancel; bounded wait |
| Cache file corrupt | Start empty; log |
| Log spam | Rate-limit per node/reason per minute |
| Selected node changes mid-probe | Discard L3 result |

## 9. Resource bounds

- Max nodes per cycle: 2,000; max groups: 100.
- L1 concurrency 10; L2 concurrency 5; one cycle at a time.
- Cache 5,000 entries; persistence 2 MB and at most every 5 minutes.
- Targets per level: max 5.
- No unbounded channels; use `buffer_unordered` and a fixed result vector.
- Timers use `sleep`, not polling loops.
