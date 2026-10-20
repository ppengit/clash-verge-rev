# Design: scheduled WebDAV sync for user-owned configuration

## 1. Data model

### 1.1 Subscription metadata

Normalized document, synced as `subscriptions.yaml`:

```yaml
version: 1
subscriptions:
  - sync_id: "Sabc..."
    name: "Provider A"
    url: "https://..."
    type: remote
    option:
      update_interval: 360
      allow_auto_update: true
      user_agent: null
      timeout_seconds: 60
      with_proxy: false
      self_proxy: false
      danger_accept_invalid_certs: false
    order: 0
    updated_at: 1760000000000
    deleted: false
```

`updated_at` is metadata edit time, not fetch time.

### 1.2 Overrides layout

```text
overrides/
  global/script.js
  global/merge.yaml
  <sync_id>/script.js
  <sync_id>/merge.yaml
  <sync_id>/proxies.yaml
  <sync_id>/groups.yaml
```

The entire dependency closure is synced together because custom rules may reference groups and rule-providers defined here.

### 1.3 Local sync state

`sync-state.json`:

```json
{
  "device_id": "...",
  "device_name": "...",
  "last_revision": 11,
  "last_synced_at": "...",
  "last_local_hash": "...",
  "last_error": null
}
```

Device name defaults to hostname and is editable.

### 1.4 Remote manifest

```json
{
  "schema": 1,
  "revision": 12,
  "parent_revision": 11,
  "device_id": "...",
  "device_name": "home-pc",
  "created_at": "...",
  "scopes": ["subscriptions", "overrides", "custom_rules"],
  "payload_sha256": "...",
  "payload_size": 12345,
  "encryption": {
    "algo": "aes-256-gcm",
    "kdf": "pbkdf2-sha256",
    "iterations": 200000,
    "salt": "base64"
  },
  "files": [
    { "path": "custom-rules.yaml", "sha256": "...", "size": 123 }
  ],
  "deleted": ["overrides/Sold/script.js"]
}
```

## 2. Configuration

Add `SyncSettings` to `IVerge`:

```rust
pub struct SyncSettings {
    pub enabled: Option<bool>,
    pub mode: Option<String>,             // two_way | upload | download
    pub scopes: Option<Vec<String>>,      // subscriptions | overrides | custom_rules | settings
    pub interval_minutes: Option<u64>,
    pub on_startup: Option<bool>,
    pub on_change: Option<bool>,
    pub conflict_policy: Option<String>,  // last_modified | local | remote | ask
    pub device_name: Option<String>,
    pub encrypt: Option<bool>,
    #[serde(serialize_with = "serialize_encrypted", deserialize_with = "deserialize_encrypted")]
    pub password: Option<String>,
}
```

Reuse `webdav_url`, `webdav_username`, `webdav_password` by default. Remote directory is a constant `clash-verge-sync` in v1; a field can be added later.

## 3. Sync algorithm

`SyncManager` is a Rust singleton initialized in `resolve_setup`.

### 3.1 Trigger and single-flight

- `watch` channel for settings.
- Tokio `Mutex` or atomic guard prevents concurrent syncs.
- If a trigger arrives while running, set one pending flag; run once more after completion.
- Cancellation token for app exit.

### 3.2 Read/compare

1. Fetch manifest with 15 s timeout and 1 MB limit.
2. If missing:
   - no local state -> first sync, upload.
   - local state exists -> treat as remote loss; abort.
3. Validate schema <= supported.
4. Compute local hash per scope and compare with `last_revision` / `last_local_hash`.
5. Four cases: no-op, upload, download/apply, conflict.

### 3.3 Upload

1. Build payload in a temp directory.
2. Zip with stored compression, bounded entry count and sizes.
3. Compute SHA-256.
4. Encrypt if enabled.
5. Upload payload first.
6. Write manifest with incremented revision (commit point).
7. Append history; failure is non-fatal.
8. Prune history to the newest 10 and clean orphan payloads older than 1 day.

### 3.4 Download/apply

1. Stream payload to a temp file with a 100 MB cap.
2. Verify hash.
3. Decrypt (wrong password -> abort).
4. Extract to `sync-staging/` with path traversal and zip-bomb checks.
5. Parse and validate YAML/JS.
6. Create a full local backup.
7. Merge/apply:
   - subscriptions
   - overrides
   - custom rules
8. Re-enhance current profile.
9. On failure restore the backup and re-enhance.
10. Refresh `Timer`, `NodeHealthManager`.
11. Update `sync-state.json`.

### 3.5 Merge rules

- **Subscriptions**:
  - match by `sync_id`; fallback normalized URL + type.
  - create missing remote subscriptions with a new local `uid` and the same `sync_id`.
  - update name/URL/options/order.
  - preserve local `updated`, `extra`, `selected`.
  - tombstone deletes only when newer than local metadata; if local was modified after the tombstone, create a conflict instead.
- **Overrides**: file-level last-modified; keep loser.
- **Custom rules**: per-rule last-modified; tombstones.
- **Settings** (optional): key-level whitelist; device-local keys always local.

### 3.6 Encryption

- PBKDF2-HMAC-SHA256, 200,000 iterations, random 16-byte salt.
- AES-256-GCM with random 12-byte nonce.
- Encrypt the zip bytes; manifest stores KDF parameters and payload hash of ciphertext.
- Password only local and encrypted with existing `serialize_encrypted`.
- No password -> plaintext with an explicit UI warning.

## 4. Safety and error handling

| Failure | Behavior |
|---|---|
| Offline/DNS | Record error; backoff 1/5/15 min; no local changes |
| 401/403 | Stop; notify credentials; no retry |
| TLS error | Stop; suggest certificate fix; optional invalid-cert opt-in |
| 507/storage full | Stop; notify |
| Manifest corrupt | Stop; keep local |
| Payload hash mismatch | Delete temp; stop |
| Wrong password | Stop before staging apply |
| Zip traversal/bomb | Stop; delete staging |
| Config validation failure | Restore local backup; re-enhance; report |
| Crash during apply | Startup cleans staging; target files are atomic; backup remains |
| History write failure | Non-fatal |
| Remote subscription fetch failure | Keep/create profile; mark "needs valid URL"; do not delete local content |
| Concurrent devices | Last writer wins manifest; other device detects revision change and creates conflict copies |
| Clock skew | revision tiebreak; conflict copies retained |

## 5. WebDAV client changes

- Reuse `core/backup.rs` transport but extract shared helpers.
- TLS verification by default; optional invalid certificates with warning (current client accepts invalid certs unconditionally).
- Optional system proxy support.
- Custom remote directory constant.
- Bounded retry: 3 attempts with exponential backoff; no retry for 401/403.
- Reuse existing XML list parsing and timeout patterns.

## 6. Scheduling

- Startup: wait for resolve + 60 s jitter.
- Interval: minimum 15 min, default 360 min.
- On-change: debounce 60 s; default off.
- Backoff on failure; reset on success.
- No sync starts after app exit; in-flight requests have timeouts.

## 7. Resource bounds

| Resource | Limit |
|---|---|
| Subscriptions | 500 |
| Override files | 100 |
| Custom rules | 10,000 |
| Script size | 5 MB each |
| Payload | 100 MB (expected < 5 MB) |
| Uncompressed archive | 200 MB |
| Archive entries | 2,000 |
| History | 10 |
| Conflict copies | 20 |
| Manifest | 1 MB |

- Streaming upload/download; no whole-payload buffering.
- Single-flight; one pending rerun at most.
- Bounded retries and timeouts.

## 8. UI

New `sync-viewer.tsx`:

- Enable toggle.
- WebDAV connection fields / "use backup credentials" toggle / test connection.
- Scope checkboxes.
- Mode and interval.
- Conflict policy.
- Optional password.
- Status: last sync, last error, device list, "Sync now".
- History viewer with restore.
- Conflict dialog listing records/files and retained copies.

## 9. Dependencies

- `PrfItem.sync_id` from child 1.
- `custom-rules.yaml` schema from child 1.
- Settings structs from child 2 if the optional settings scope is enabled.
- Existing WebDAV credentials and backup primitives.
