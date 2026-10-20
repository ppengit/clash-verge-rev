# Scheduled WebDAV sync for user-owned configuration

## Goal

Synchronize user-owned configuration across devices over WebDAV without uploading fetched subscription proxy content. Default scope: subscription metadata (name, URL, update options), user-authored overrides, and custom rules. Settings are an optional fourth scope.

## User stories

- As a multi-device user, I can configure a WebDAV account once and have my subscriptions, scripts, and custom rules appear on another device.
- As a user, I do not need to upload or download the actual proxy content; each device fetches subscriptions from its own URL.
- As a user, when two devices edit the same item, the most recently modified version wins and the older version is recoverable.
- As a user, I can see when the last sync happened, what failed, and restore a previous cloud version.
- As a user, I can optionally protect the cloud payload with a password, but I am not forced to.
- As a user, a scheduled sync never restarts my app or overwrites device-specific settings.

## Requirements

### Scope

Default sync scopes:

1. **Subscriptions**: `sync_id`, name, URL, type, update interval, `allow_auto_update`, user agent, timeout, proxy options, order, `updated_at`, deletion tombstone.
2. **Overrides**: global and per-profile Script/Merge/Proxies/Groups authored by the user. Include the whole dependency closure used by custom rules.
3. **Custom rules**: `custom-rules.yaml`.

Optional fourth scope: **Settings** whitelist, default off.

Never sync:

- `profiles/R<uid>.yaml` or provider caches
- traffic/expiry or fetch timestamps
- WebDAV credentials or `.encryption_key`
- ports, TUN, system proxy, hotkeys, window state, device-local settings

### Remote layout

```text
<WebDAV>/clash-verge-sync/
  manifest.json
  payload.zip
  history/<revision>-<device>.zip
```

`manifest.json` includes schema, revision, parent revision, device identity, created time, scopes, payload hash/size, encryption parameters, file hashes, and deletion list.

### Conflict policy

- Default: last-modified-wins at record/file granularity.
- Subscriptions: per `sync_id` (fallback normalized URL + type).
- Overrides: per file.
- Custom rules: per rule `id`.
- Deletions use tombstones.
- The losing side is copied to `conflicts/<time>-<device>/`; remote history retains recent versions.
- Equal/missing timestamps use manifest revision; final tie goes to local with a remote conflict copy.

### Apply safety

- Download and verify before applying.
- Stage all files; validate YAML/JS syntax.
- Create a full local backup before apply.
- Apply atomically; on failure restore the backup and re-enhance.
- Re-enhance the current profile; refresh `Timer` and health cache.
- Do not auto-restart. If a restart-required setting is synced, notify.

### Security

- Optional password encryption: PBKDF2-HMAC-SHA256 + AES-256-GCM.
- Password stored locally with existing encryption; never synced.
- TLS verification by default; invalid certificates only with explicit opt-in.
- Zip path traversal and zip-bomb protection.
- Redact subscription URLs and secrets from logs.

### Scheduling

- Startup after resolution with jitter.
- Configurable interval, minimum 15 minutes.
- Optional on-change trigger with debounce.
- Single-flight; skip or coalesce concurrent triggers.
- Bounded retry/backoff; no retry on 401/403.

## Acceptance criteria

- [ ] Two devices can sync subscription metadata, overrides, and custom rules without uploading fetched proxy content.
- [ ] A new device can recreate remote subscriptions from metadata and fetch content locally.
- [ ] Both sides modifying different records merges without losing either side's changes.
- [ ] Both sides modifying the same record resolves by last modified and retains the loser.
- [ ] Deletions propagate via tombstones and do not resurrect.
- [ ] A wrong password, corrupt manifest, hash mismatch, or zip bomb aborts without changing local files.
- [ ] A config validation failure after apply restores the local backup and re-enhances successfully.
- [ ] A scheduled sync never restarts the app.
- [ ] Device-local settings and WebDAV credentials are never overwritten.
- [ ] Remote history is capped and old snapshots are pruned.
- [ ] A 24-hour sync soak shows bounded memory, handles, and history growth.

## Non-goals

- Uploading fetched subscription proxy content.
- Full three-way merge for scripts or YAML.
- Syncing all settings by default.
- Automatic deletion of local profiles with unsynced modifications.
- Multi-user shared editing or fine-grained ACLs.
- Atomic compare-and-swap across devices; last writer wins the manifest, with conflict detection on the next sync.

## Constraints

- Depends on child 1 `sync_id` and child 2 settings schema.
- Reuse `WebDavClient`/`reqwest_dav` and existing backup primitives.
- All writes must be staged/atomic and bounded.
- No unbounded retry loops, payloads, history, or conflict directories.
- Follow repository and backend guardrails.
