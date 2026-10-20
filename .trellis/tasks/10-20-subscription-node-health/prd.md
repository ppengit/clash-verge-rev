# Subscription and node settings with automated health verification

## Goal

Give users one place to control subscription updates, automated node testing, and node visibility, and make automated tests verify that a node can reach real websites rather than only returning a fast `generate_204` response.

## User stories

- As a user, I can open Settings and see a "Subscription & Nodes" section instead of hunting through profile dialogs and Misc settings.
- As a user, I can set a default update interval for new subscriptions, choose whether updates run on startup, and only update the current subscription.
- As a user, I can enable periodic node testing that runs in the background even when I am not on the Home page.
- As a user, I can see that a 30 ms node is actually unusable because it returns 403 from a real site, not just because it has high latency.
- As a user, I can hide timeout/error/unhealthy nodes globally and override that per proxy group from the group toolbar.
- As a user, when I select a bad node, I get an immediate warning and a suggestion instead of a silently broken browser.
- As an existing user, my old "Auto Delay Detection" setting keeps working after migration.

## Requirements

### Settings section

Add a Settings card "Subscription & Nodes" with four sections:

1. **Subscription updates**
   - default update interval for new subscriptions
   - update on startup
   - update only current subscription
   - notify on failure
2. **Scheduled testing & health**
   - enable/disable
   - L1 interval (link/latency)
   - L2 interval (real-site status)
   - scope: current node / current group / all groups
   - L1/L2 targets with expected status
   - verify on selection
   - auto-failover (optional, default off)
3. **Node display**
   - hide timeout nodes
   - hide untested nodes
   - hide unhealthy nodes
   - hide nodes above N ms
   - default sort
4. **Subscription information**
   - show traffic/expiry
   - expiry warning days

The Profiles page gets a gear icon that opens the same dialog.

### Health verification

- Run a Rust `NodeHealthManager`, not a frontend timer.
- Use mihomo per-node delay APIs with `expected` status:
  - `/proxies/{name}/delay?url=...&timeout=...&expected=...`
  - `/providers/proxies/{provider}/{node}/healthcheck?...`
- L1: neutral HTTPS 204 endpoints, expected 204.
- L2: real-site URLs, expected 200 or 200-299.
- L3: selected-node content probe through the local proxy, checking status and optional keyword.
- Health states: `healthy`, `degraded`, `unusable`, `site_blocked`, `unverified`, `testing`.
- Selected-node verification is asynchronous and must not block switching.
- Auto-failover is opt-in, with cooldown and a per-hour switch cap.

### Node display

- Global defaults in `NodeDisplaySettings`.
- Per-group override in `HeadState.hideUnavailable`, persisted in localStorage.
- New icon in `ProxyGroupTools`; do not reuse the existing detail/visibility icon.
- Filtering is display-only; do not modify mihomo `filter`/`exclude-filter`.
- Pin the current node even when it would be hidden.
- Show hidden count; provide "show all" when empty.
- Search bypasses the hide filter.
- Manual group tests still test hidden nodes.

### Migration

- `enable_auto_delay_detection` maps to health checks enabled with scope `current_node` and the old interval.
- Old fields remain for one release; old UI is hidden.
- `default_latency_test` and `default_latency_timeout` move into the new section while keeping their keys.

## Acceptance criteria

- [ ] The Settings page shows "Subscription & Nodes"; the Profiles page gear opens the same dialog.
- [ ] Settings persist across restart and invalid values are rejected with inline messages.
- [ ] Background L1/L2 checks run while the app is on any page and while the window is minimized.
- [ ] A node that passes L1 but returns 403 from a real site is marked `site_blocked`, not `healthy`.
- [ ] A node that fails HTTPS 204 is marked `unusable` and sorted below healthy nodes.
- [ ] A node with high jitter/low success rate is marked `degraded`.
- [ ] Selecting an unusable node triggers an immediate real-site verification and a warning with alternatives.
- [ ] The per-group toolbar icon toggles hiding; the global default applies when the group has no override.
- [ ] The current node remains visible and pinned when the filter is on.
- [ ] A group whose nodes are all hidden shows a "show all" empty state.
- [ ] Search still finds a hidden node.
- [ ] Manual group testing still tests hidden nodes.
- [ ] Old `enable_auto_delay_detection` settings migrate without user action.
- [ ] A 24-hour soak shows bounded memory, handles, and log growth.

## Non-goals

- Full content validation for every node in every background cycle. v1 performs L3 only for the selected node; all-node tests use L1/L2.
- A headless browser page-render test.
- Modifying mihomo group filters to hide nodes.
- Guaranteeing that every target website is reachable; site-specific blocks remain inherent.
- Auto-failover enabled by default.

## Constraints

- Do not hold configuration locks across network awaits.
- Use bounded concurrency and request timeouts.
- Cancel stale cycles when the profile, group set, or settings change.
- Keep all new config fields optional.
- Follow repository and backend/frontend guardrails.
