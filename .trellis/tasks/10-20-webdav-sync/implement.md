# Implementation plan: scheduled WebDAV sync

## Order

1. Sync config and local state
2. Payload build/verify and manifest
3. Subscription metadata read/merge
4. Overrides and custom-rule sync
5. Apply/rollback and scheduler
6. Conflicts, history, and UI
7. Optional encryption and TLS/proxy options
8. Integration and soak tests

## 1. Foundation

- [ ] Add `SyncSettings` to `IVerge` with field-level merge and defaults.
- [ ] Add `sync-state.json` read/write with atomic replace.
- [ ] Generate `device_id`; default `device_name` from hostname.
- [ ] Add `sync/` directory constants and staging/conflict paths.
- [ ] Add commands: `get_sync_status`, `sync_now`, `test_webdav`, `list_sync_history`, `resolve_sync_conflict`.

## 2. Payload and manifest

- [ ] Build `subscriptions.yaml` from local profiles (metadata only).
- [ ] Collect user-authored overrides and `custom-rules.yaml`.
- [ ] Zip with bounds and SHA-256.
- [ ] Read/write/validate `manifest.json`.
- [ ] Implement schema rejection for newer versions.
- [ ] Implement payload hash verification and bounded download.

## 3. Merge

- [ ] Match subscriptions by `sync_id`, fallback normalized URL + type.
- [ ] Create missing local profiles with new `uid` and preserved `sync_id`.
- [ ] Preserve local fetch timestamps, traffic, expiry, and selection.
- [ ] Merge overrides per file with conflict copies.
- [ ] Merge custom rules per id with tombstones.
- [ ] Implement deletion rules and tombstones.

## 4. Apply and rollback

- [ ] Stage archive extraction with path traversal/zip-bomb checks.
- [ ] Validate YAML/JS syntax before apply.
- [ ] Create a full local backup.
- [ ] Apply subscriptions/overrides/custom rules atomically.
- [ ] Re-enhance the current profile.
- [ ] Restore backup on failure and re-enhance.
- [ ] Refresh `Timer` and `NodeHealthManager`.

## 5. Scheduler

- [ ] Add `core/sync.rs` singleton and init in `resolve_setup`.
- [ ] `watch + sleep` with min interval, startup jitter, backoff.
- [ ] Single-flight and one pending rerun.
- [ ] Cancellation on app exit.
- [ ] Optional on-change trigger from `save_profile_file` and profile CRUD with 60 s debounce.

## 6. UI and security

- [ ] Add Settings card/dialog.
- [ ] Add test-connection action.
- [ ] Add conflict dialog and history viewer.
- [ ] Add optional password encryption and warning when empty.
- [ ] Add TLS verification default and optional proxy.
- [ ] Redact logs.

## 7. Tests

- [ ] Unit: manifest validation, record-level merge, tombstones, equal timestamps, zip safety.
- [ ] Integration: local WebDAV server, two app instances, conflict scenarios.
- [ ] Failure injection: offline, 401, corrupt manifest, hash mismatch, wrong password, config validation failure.
- [ ] Soak: 24 hours with interval sync; bounded memory, handles, history.
- [ ] Verify no fetched `R<uid>.yaml` is uploaded.

## Validation

```bash
pnpm typecheck
pnpm lint
pnpm test -- <sync targeted tests>
pnpm gen:contract
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml sync
```

## Review gates

- [ ] No payload contains fetched subscription content or WebDAV credentials.
- [ ] No apply without staging, hash verification, and a local backup.
- [ ] No scheduled sync restarts the app.
- [ ] No unbounded retry/history/conflict growth.
- [ ] Device-local settings are preserved.
- [ ] A newer schema is rejected, not partially applied.

## Rollback points

- Payload upload precedes manifest; a failed manifest write leaves the previous manifest authoritative.
- Apply keeps a full local backup; failure restores and re-enhances.
- Staging is separate from live directories; startup cleans leftovers.
- Encryption can be disabled by clearing the password; existing plaintext payloads remain readable.
