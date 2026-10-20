# Implementation plan: standalone custom rules and quick rule creation

## Order

1. `sync_id` foundation
2. Custom rules storage and enhancement
3. Commands and validation
4. Quick-add UI and Rules page
5. Connections suggestions
6. Subscription-update preflight
7. Tests and migration checks

## 1. `sync_id`

- [ ] Add `sync_id: Option<String>` to `PrfItem`.
- [ ] Generate on new profiles.
- [ ] Add lazy migration for existing profiles with atomic `profiles.yaml` save.
- [ ] Add `ensure_profile_sync_id`.
- [ ] Ensure profile import/duplicate/patch preserves `sync_id`.
- [ ] Test migration and save failure.

## 2. Storage and enhancement

- [ ] Add `custom_rules_path()` and file name constant.
- [ ] Add `enhance/custom_rules.rs`: schema, cache, load, save, validation, injection.
- [ ] Add write lock and atomic write with `.bak`.
- [ ] Call injection after Script and before cleanup in `enhance/mod.rs`.
- [ ] Skip invalid references and record warnings.
- [ ] Add unit tests for prepend/append-before-MATCH and invalid references.

## 3. Commands

- [ ] Add `cmd/custom_rules.rs` with list/get targets/ensure/add/bulk/delete/validate.
- [ ] Register in `cmd/mod.rs` and `ipc.rs`.
- [ ] Add error codes for profile missing, policy missing, duplicate, limit, parse failure.
- [ ] Run `pnpm gen:contract` and add `services/cmds.ts` wrappers.

## 4. Quick-add UI

- [ ] Extract rule metadata from `rules-editor-viewer.tsx` to `components/rule/rule-meta.ts`.
- [ ] Add `quick-add-rule-dialog.tsx`.
- [ ] Add Rules page header button.
- [ ] Add RTL "Add rule" empty state.
- [ ] Add source badges, filter chips, and row actions.
- [ ] Add undo toast.
- [ ] Add i18n keys and run `pnpm i18n:types`.

## 5. Connections

- [ ] Add context-menu/detail action to `connection-table.tsx`, `connection-row-item.tsx`, or `connection-detail.tsx`.
- [ ] Build suggestions from `metadata.host`, `destinationIP`, `process`, `processPath`, `destinationPort`, `network`.
- [ ] Open the quick-add dialog prefilled.

## 6. Subscription-update preflight

- [ ] Stage downloaded profile content.
- [ ] Parse groups/providers/sub-rules.
- [ ] Validate custom and legacy rules.
- [ ] Return `ProfileUpdateConflict` without writing.
- [ ] Add remap dialog and apply-fixes command.
- [ ] Timer path: keep old profile, notify, retry at normal interval.
- [ ] Test cancel, remap, delete, DIRECT fallback, and Script-generated groups.

## Validation

```bash
pnpm typecheck
pnpm lint
pnpm test -- src/services/delay.test.ts src/components/proxy/use-filter-sort.test.ts
pnpm i18n:types
pnpm gen:contract
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml custom_rules
```

## Review gates

- [ ] No profile is written before validation succeeds.
- [ ] Malformed rule file cannot prevent app startup.
- [ ] Existing legacy rules still apply.
- [ ] No unbounded cache/list added.
- [ ] All new commands have typed errors and frontend handling.

## Rollback points

- Rule write: restore previous `custom-rules.yaml` on enhance failure.
- Profile update: previous profile bytes remain until validation succeeds.
- Migration: `sync_id` save failure leaves the profile otherwise unchanged.
