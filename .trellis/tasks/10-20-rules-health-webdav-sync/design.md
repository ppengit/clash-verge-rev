# Design: custom rules, node health, and WebDAV sync

## 1. Architecture boundaries

The design separates three data classes:

| Class | Examples | Owner | Synced |
|---|---|---|---|
| User-owned configuration | subscription metadata, `custom-rules.yaml`, user-authored Script/Merge/Proxies/Groups, selected settings | Application | Yes |
| Fetched subscription data | `profiles/R<uid>.yaml`, provider caches, traffic/expiry, fetch timestamps | Config/profile layer | No |
| Device-local state | ports, TUN, system proxy, hotkeys, window state, `sync-state.json`, health cache, WebDAV credentials | Application/device | No |

The central identity is `PrfItem.sync_id`. It is independent from the local `uid` and is the key used by custom rules and sync records.

## 2. Reused infrastructure

- `Config` draft/apply and `CONFIG_WRITE_LOCK` for Verge/profile writes.
- `enhance::collect_profile_items` and `process_seq_items` for config generation.
- `save_profile_file` and `CoreConfigValidator` for validation/apply/rollback.
- `WebDavClient` and `reqwest_dav` for transport.
- `Timer` and `AutoBackupManager` `watch + sleep` patterns for schedulers.
- `specta` command registration and `pnpm gen:contract`.
- `delayManager` snapshot/listener pattern for UI state.
- `HeadState` localStorage for per-profile/per-group UI state.

## 3. Data flow

### Rule creation

```text
Quick-add dialog
  -> target profile sync_id + rule fields
  -> add_custom_rule command
  -> validate target/profile/rule/policy/references/duplicate/limits
  -> atomic write custom-rules.yaml
  -> if current profile: enhance/apply; on failure restore previous file
  -> refresh runtime rules + proxy view
```

### Enhancement

```text
subscription config
  -> legacy rules seq (r<uid>.yaml)
  -> Merge
  -> Script
  -> custom-rules injection (prepend; append before first MATCH)
  -> cleanup/sort
  -> validate/apply core config
```

Custom rules are injected after Script so they can reference Script-generated groups. Invalid references are skipped with a warning; the config is not failed.

### Node health

```text
NodeHealthManager (Rust, single scheduler)
  -> snapshot profile/group/nodes + generation
  -> L1/L2 probes via mihomo delay API with expected status
  -> selected-node L3 content probe via local proxy
  -> health state map
  -> event/snapshot to frontend
  -> badges, sorting, filtering, selected-node warning
```

### Sync

```text
SyncManager (Rust, single-flight)
  -> local user-owned payload
  -> remote manifest (revision, hashes, scopes)
  -> compare revision + local hash
  -> upload payload then manifest, or download/verify/stage/apply
  -> record-level last-modified merge + conflict copies
  -> re-enhance current profile, refresh Timer/health cache
```

## 4. Cross-cutting failure model

- **Validation before commit.** New rule writes, profile updates, and sync apply are validated or staged before replacing live files.
- **Rollback.** Keep previous bytes in memory or a backup; restore and re-enhance on failure.
- **No panics on input.** Malformed YAML/JSON, missing files, unknown schema, and invalid URLs return errors or are skipped with logs.
- **Generation/cancellation.** Background tests and sync carry a generation; profile changes, settings changes, app exit, or a newer run cancel older work.
- **Bounded collections.** Every map, queue, history directory, and cache has a documented maximum.
- **Bounded I/O.** Network responses, archive entries, file reads, retries, and log warnings have limits.
- **No new unbounded task spawns.** Use `buffer_unordered` with fixed concurrency or a single loop; no unbounded channels.

## 5. Compatibility and migration

- All new config fields are optional; missing fields keep current behavior.
- `sync_id` is generated lazily and saved back; failure does not block startup.
- Legacy `r<uid>.yaml` rules remain active; the new file is additive.
- `enable_auto_delay_detection` is migrated to the health manager and retained as a compatibility field for one release.
- Old backups remain valid; new backups include `custom-rules.yaml`.
- Sync schema is versioned; future versions are rejected with a clear message.

## 6. Rollout

1. Add `sync_id` and the custom-rules foundation behind no UI.
2. Add quick-add UI and subscription-update preflight.
3. Add settings and health manager; migrate the old auto-delay setting.
4. Add sync manifest/scheduler, then merge/conflict, then encryption.
5. Add diagnostics and optional deep tests last.

Each child task is independently reviewable. Child 3 must not start before child 1 has a stable `sync_id` and child 2 has frozen its settings schema.

## 7. Detailed child designs

See:

- `../10-20-custom-rules-quick-add/design.md`
- `../10-20-subscription-node-health/design.md`
- `../10-20-webdav-sync/design.md`
