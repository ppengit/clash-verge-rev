# Implementation plan (parent)

## Preconditions

- User has approved this task tree and reviewed the child plans.
- `task.py start` is only run after review. Creating the task is not approval to implement.
- Each child gets its own issue and PR(s); no drive-by refactors.

## Phase 0: Specification freeze

- [ ] Confirm `PrfItem.sync_id` schema and migration behavior.
- [ ] Confirm `custom-rules.yaml` schema, limits, and enhancement order.
- [ ] Confirm `IVerge` nested settings structs and patch semantics.
- [ ] Confirm sync scopes, remote layout, conflict policy, and encryption behavior.
- [ ] Record the decisions in the child `design.md` files.

## Phase 1: Custom rules foundation and quick add

Child: `10-20-custom-rules-quick-add`

- [ ] Add `sync_id` to `PrfItem`, generation, lazy migration, and save-back.
- [ ] Add `custom-rules.yaml` storage, cache, atomic write, limits, and validation.
- [ ] Inject custom rules in `enhance()` after Script and before cleanup.
- [ ] Add custom-rule commands and `get_rule_targets`.
- [ ] Add quick-add dialog on the Rules page.
- [ ] Add connection context-menu suggestions.
- [ ] Add source badges and custom-rule actions on the Rules page.
- [ ] Add subscription-update compatibility preflight and remap dialog.
- [ ] Add targeted tests for rule parsing/injection and update preflight.

Validation gates:

```bash
pnpm typecheck
pnpm lint
pnpm test -- <targeted tests>
pnpm i18n:types
pnpm gen:contract
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml <targeted tests>
```

## Phase 2: Subscription and node health

Child: `10-20-subscription-node-health`

- [ ] Add `SubscriptionSettings`, `NodeHealthSettings`, `NodeDisplaySettings` to `IVerge`.
- [ ] Add field-level patch merge and registry entries.
- [ ] Add the Settings page card and four-section dialog.
- [ ] Add expected-status support for per-node delay probes.
- [ ] Add `NodeHealthManager` with L1/L2, generation cancellation, bounded concurrency, cache, and persistence.
- [ ] Add selected-node L3 content verification.
- [ ] Add health badges, sorting, and filtering.
- [ ] Add `HeadState.hideUnavailable`, the group toolbar icon, pinned current node, hidden count, and search bypass.
- [ ] Migrate `enable_auto_delay_detection` and hide the old UI.
- [ ] Add targeted tests for health state and filter composition.

Validation gates: same as Phase 1, plus a 24-hour soak in a development build.

## Phase 3: WebDAV sync

Child: `10-20-webdav-sync`

- [ ] Add sync config fields, sync state, and device identity.
- [ ] Add manifest read/write, payload build, and hashes.
- [ ] Add `SyncManager` scheduler with startup/interval/on-change triggers, single-flight, backoff, and cancellation.
- [ ] Add subscription metadata normalization and merge.
- [ ] Add override and custom-rule sync.
- [ ] Add apply staging, local backup, re-enhance, and rollback.
- [ ] Add tombstones, conflict copies, and history retention.
- [ ] Add optional password encryption and TLS/proxy options.
- [ ] Add conflict/history UI.
- [ ] Add targeted tests for the sync state machine, tombstones, zip safety, and equal-timestamp handling.

Validation gates: same as Phase 1, plus a two-instance/local-WebDAV integration matrix and a 24-hour sync soak.

## Phase 4: Diagnostics and polish

- [ ] Add staged connection diagnosis (DNS/TCP/TLS/HTTP).
- [ ] Add optional site-reachability matrix and deep test UI.
- [ ] Update `template/Changelog.md`.
- [ ] Update Trellis specs with learned conventions.
- [ ] Run final check and archive children before the parent.

## Rollback points

- Each rule write restores the previous `custom-rules.yaml` on enhance failure.
- Each profile update keeps the previous profile bytes until validation succeeds.
- Sync creates a full local backup and a staging directory before apply; failure restores both.
- No scheduler starts a new cycle after app exit; in-flight work is bounded by timeouts.

## Completion criteria

- All child acceptance criteria pass.
- No feature introduces unbounded queues, caches, retries, or history directories.
- Existing configs, profiles, backups, and old rules still work.
- Cross-child integration is verified on two profiles and two devices.
