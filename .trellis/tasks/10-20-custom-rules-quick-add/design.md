# Design: standalone custom rules and quick rule creation

## 1. Data model

### 1.1 `PrfItem.sync_id`

```rust
pub struct PrfItem {
    pub sync_id: Option<String>,
    // existing fields
}
```

- Generated with `help::get_uid("S")`.
- Generated on new profiles and lazily for existing profiles.
- Persisted in `profiles.yaml`; failure is logged and retried, not fatal.
- Used by `custom-rules.yaml` and WebDAV sync.

### 1.2 `custom-rules.yaml`

Path: `dirs::app_home_dir()?.join("custom-rules.yaml")`.

```yaml
version: 1
rules:
  - id: "S01J..."
    profile: "Sabc..."
    position: prepend
    type: DOMAIN-SUFFIX
    value: example.com
    policy: "🚀 节点选择"
    policy_kind: group
    no_resolve: false
    enabled: true
    created_at: 1760000000000
    updated_at: 1760000000000
    deleted: false
```

Rules:

- `position: append` means "insert before the first MATCH".
- `profile: "*"` is allowed; v1 requires `policy_kind = builtin`.
- `deleted: true` is a sync tombstone; compaction after 90 days.
- Invalid entries are skipped; the rest load.

### 1.3 Limits

| Limit | Value |
|---|---|
| File size | 2 MB |
| Rule count | 10,000 |
| `value` length | 2,048 |
| `policy` length | 256 |
| Warning rate | Once per rule/reason per minute |

## 2. Storage and cache

- New module `enhance/custom_rules.rs`.
- `CUSTOM_RULES_WRITE_LOCK: tokio::sync::Mutex<()>`.
- Read cache: `RwLock<Option<CustomRulesCache>>` with `mtime`, `size`, `Arc<CustomRules>`.
- On read error: return last good cache and log; if none, return empty rules.
- On write: serialize under lock, write temp file, `rename` over the target, invalidate cache.
- Keep a `.bak` of the previous valid file for manual recovery.

## 3. Commands

New `cmd/custom_rules.rs`:

```text
list_custom_rules(profile_uid) -> Vec<CustomRuleView>
get_rule_targets(profile_uid) -> RuleTargets
ensure_profile_sync_id(uid) -> String
add_custom_rule(payload) -> CustomRuleView
add_custom_rule_bulk(payload, mappings) -> Vec<CustomRuleView>
delete_custom_rule(id) -> ()
validate_custom_rules(profile_uid) -> Vec<RuleIssue>
```

`RuleTargets` contains effective groups, rule-provider names, sub-rule names, and built-ins.

Validation order:

1. Target profile exists and has `sync_id`.
2. Type is known; `value`/`policy` non-empty and within limits.
3. `policy_kind` matches built-in or effective group.
4. `RULE-SET` provider exists; `SUB-RULE` exists.
5. No exact duplicate in the same profile + position.
6. File/rule count limits not exceeded.

## 4. Enhancement injection

Current order in `enhance::enhance()`:

```text
process_seq_items (legacy rules/proxies/groups)
-> merge_default_config
-> global merge/script
-> profile merge/script
-> authoritative enforcement
-> cleanup/sort
```

New step after profile script and before authoritative enforcement/cleanup:

```rust
config = apply_custom_rules(config, current_profile_sync_id).await?;
```

Algorithm:

1. Load enabled, non-deleted rules for `sync_id` or `*`.
2. Build rule strings.
3. `prepend`: prepend to `rules`.
4. `append`: find the first string whose first field is `MATCH` (case-insensitive); insert before it; otherwise append.
5. Validate policy/provider/sub-rule against the current config; skip and record issues.
6. On unexpected error: return the original config and log; never fail the whole enhancement.

## 5. Quick-add UI

New `components/rule/quick-add-rule-dialog.tsx`.

- Props: `open`, `defaultProfileUid`, optional `prefill`, `onClose`, `onAdded`.
- Target selector: current profile by default; "Advanced" expands other profiles.
- Rule metadata extracted from `rules-editor-viewer.tsx` into `components/rule/rule-meta.ts`.
- Policy combo:
  - Current profile: `useProxiesData().proxyView`.
  - Other profile: `get_rule_targets` command.
  - `freeSolo` only for non-current profiles with a Script.
- Client validation mirrors backend; backend remains authoritative.
- Submit: `mutate(add_custom_rule, { id: "add-custom-rule" })`; disable while pending.
- Undo: `delete_custom_rule(id)`.

Rules page:

- Header button.
- `list_custom_rules` + runtime rules matching by exact rule string.
- Source badges and filter chips.
- Row menu for custom rules.

Connections:

- Context menu / detail action builds suggestions:
  - domain -> `DOMAIN-SUFFIX`
  - IP -> `IP-CIDR` (`/32` or `/128`)
  - process -> `PROCESS-NAME`
  - path -> `PROCESS-PATH`
  - port -> `DST-PORT`
- Opens the same dialog with `prefill`.

## 6. Subscription update preflight

Modify `feat/profile.rs::perform_profile_update`:

1. Fetch new content into memory/temp; do not overwrite `R<uid>.yaml`.
2. Parse YAML; on parse failure keep old profile.
3. Extract effective groups/providers/sub-rules.
4. Validate custom rules and legacy rules.
5. If invalid, return `ProfileUpdateConflict { issues }`.
6. Frontend dialog:
   - remap policy
   - change to DIRECT/REJECT
   - delete rule
   - cancel update
7. Apply fixes, re-validate, then write the new profile and apply.
8. Timer updates: no dialog; keep old profile, emit notice, retry at the normal interval.

## 7. Error handling

| Failure | Behavior |
|---|---|
| Malformed custom-rules YAML | Use last good cache; log; do not overwrite |
| Missing profile/sync_id | Return error; generate sync_id first |
| Policy/provider missing | Structured error; no write |
| Duplicate rule | Return existing id; no write |
| Limit exceeded | Reject with limit message |
| Disk full/read-only | Atomic write fails; old file intact |
| Enhance failure after write | Restore previous bytes; re-enhance; return error |
| App exit during write | Temp file may remain; startup cleanup; target is old or new |
| Subscription update conflict | Keep old profile; prompt or notify |

## 8. Resource bounds

- One file read per enhancement unless `mtime/size` changed.
- Rule string construction is O(n) with n <= 10,000.
- No per-rule timers or subscriptions.
- Warning logs rate-limited by rule id + reason.
- UI lists use existing virtualization/search components.

## 9. Migration

- Existing profiles get `sync_id` lazily.
- Legacy rules remain active; no automatic migration.
- Optional later "Migrate legacy rules" action reads `r<uid>.yaml` and offers a preview before adding to `custom-rules.yaml`.
- Old backups remain valid; new backups include the new file.
