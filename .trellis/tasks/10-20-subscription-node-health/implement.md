# Implementation plan: subscription and node settings, automated health verification

## Order

1. Settings schema and patch merge
2. Expected-status probe support
3. `NodeHealthManager` and cache
4. Frontend health state and badges
5. Node display filter and toolbar icon
6. Selected-node verification and migration
7. Soak and performance validation

## 1. Settings schema

- [ ] Add `SubscriptionSettings`, `ProbeTarget`, `NodeHealthSettings`, `NodeDisplaySettings` in `config/verge.rs`.
- [ ] Add optional `IVerge` fields and defaults.
- [ ] Implement field-level patch merge.
- [ ] Add `verge_registry!` entries with `NoEffect`.
- [ ] Refresh managers in `feat/executor.rs`.
- [ ] Add Settings card and four-section dialog.
- [ ] Add Profiles page gear entry.
- [ ] Add i18n keys and run `pnpm i18n:types`.

## 2. Expected-status probes

- [ ] Add a Rust command that sends `expected` to mihomo delay/provider endpoints.
- [ ] Return structured results (ok, delay, error) without panics.
- [ ] Use `expected=204` for L1 and `expected=200`/`200-299` for L2.
- [ ] Register in `ipc.rs` and regenerate bindings.

## 3. Health manager

- [ ] Add `core/node_health.rs` singleton.
- [ ] Initialize in `resolve_setup`; refresh on settings/profile changes.
- [ ] Implement generation, cancellation, single-flight, bounded concurrency.
- [ ] Implement L1/L2 scheduling and health state computation.
- [ ] Add cache pruning, persistence throttle, and corruption recovery.
- [ ] Emit health snapshot events.
- [ ] Add `cmd/node_health.rs` for status, deep test, and targets.

## 4. L3 content probe

- [ ] Add `core/probe.rs` using the local mixed port.
- [ ] Check status, keyword, TTFB, total time, body size, with 64 KB cap.
- [ ] Run on selected node only; discard if selection changes.
- [ ] Return structured errors for timeout/TLS/status.

## 5. Frontend

- [ ] Add health state map/listener in `services/node-health.ts`.
- [ ] Add badges to `proxy-item.tsx` / `proxy-item-mini.tsx`.
- [ ] Add health-first sorting and filters in `use-filter-sort.ts`.
- [ ] Add hidden count and "show all" state.
- [ ] Add selected-node verification in `proxy-groups.tsx`.
- [ ] Add optional auto-failover with cooldown and hourly cap.

## 6. Node display

- [ ] Add `hideUnavailable` to `HeadState`.
- [ ] Add toolbar icon in `proxy-group-tools.tsx`.
- [ ] Apply global defaults and per-group override.
- [ ] Pin current node; bypass hide filter during search.
- [ ] Keep manual test-all covering hidden nodes.

## 7. Migration and compatibility

- [ ] Migrate `enable_auto_delay_detection`.
- [ ] Hide old Misc controls.
- [ ] Move latency URL/timeout controls to the new section.
- [ ] Verify old config without new fields still works.

## Validation

```bash
pnpm typecheck
pnpm lint
pnpm test -- src/components/proxy/use-filter-sort.test.ts src/services/delay.test.ts
pnpm i18n:types
pnpm gen:contract
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml node_health
```

## Review gates

- [ ] No network call holds a config lock.
- [ ] No cycle runs concurrently with itself.
- [ ] Stale results cannot overwrite a newer profile/group snapshot.
- [ ] Cache and persistence are bounded.
- [ ] Filtering is display-only.
- [ ] The old auto-delay setting migrates without user action.

## Rollback points

- Health settings are additive; disabling them restores prior UI behavior.
- The old auto-delay fields remain readable.
- Filtering can be turned off globally or per group without touching core config.
