# Clash Verge Rev: custom rules, node health, and WebDAV sync

## Goal

Make three related configuration workflows usable and safe for multi-device users:

1. Create and manage custom rules quickly, without coupling user rules to fetched subscription content.
2. Give users subscription-level and node-level controls, including automated verification that a node can actually reach real websites.
3. Synchronize user-owned configuration across devices over WebDAV without uploading fetched subscription proxy content.

## User problems

1. **Rule creation is buried.** The Rules page is read-only; adding a rule requires finding the current profile on the Profiles page, opening its context menu, and editing an attached rules file. Users cannot start from the Rules or Connections page.
2. **Subscription groups differ.** A custom rule targeting a proxy group is only valid for the subscription that owns that group. Subscription updates can rename or remove groups, leaving stale rules and a config that fails validation.
3. **Low latency does not mean usable.** The current delay probe is a single HTTP `generate_204` request with no expected-status validation and a 30-minute cache. Nodes can look fast while failing TLS, real sites, or bandwidth requirements.
4. **No subscription/node settings section.** Auto-update defaults, scheduled testing, node visibility, and subscription information are scattered or missing.
5. **WebDAV is backup-only.** It supports manual snapshot upload/download and local auto-backup, but not scheduled two-way sync, record-level conflicts, tombstones, or optional encryption.

## Scope

### In scope

- A standalone `custom-rules.yaml` file owned by the user, keyed by a stable profile `sync_id`.
- Quick rule creation from the Rules page and Connections page.
- Subscription-update preflight for rule references, with a remap dialog.
- A "Subscription & Nodes" settings section covering update defaults, scheduled health checks, node display, and subscription information.
- A Rust `NodeHealthManager` using per-node HTTP probes with expected status codes.
- UI node filters with a global default and a per-group toolbar toggle.
- Scheduled two-way WebDAV sync for subscription metadata, user-authored overrides, and custom rules.
- Record-level last-modified conflict policy, tombstones, conflict copies, and optional password encryption.

### Out of scope

- Syncing fetched subscription proxy content (`R<uid>.yaml`), traffic counters, or expiry timestamps.
- Full three-way file merge for scripts or YAML.
- Full content validation for every node in every background cycle. v1 validates content for the selected node; all nodes use expected-status probes.
- Automatic group-policy mapping for "apply to all subscriptions" beyond built-in policies. v1 allows built-in policies for all subscriptions; group policies are per-subscription.
- Automatic application of settings that require an application restart.
- Rule hit counters or per-rule traffic statistics.

## Resolved product decisions

- Quick-add default target is the standalone `custom-rules.yaml`, not the per-profile `r<uid>.yaml`.
- Sync default scope is subscription metadata, user-authored overrides, and custom rules. Settings are an optional fourth scope, default off.
- Conflict default is last-modified-wins at record/file granularity, with losing copies retained.
- Sync password is optional; encryption is not forced.
- `enable_auto_delay_detection` is migrated into the new health manager and hidden from the old Misc dialog.
- Node filtering is display-only, with a global default and a per-group toolbar toggle.

## Cross-child requirements

- `sync_id` must be stable across devices and survive subscription URL changes.
- User-owned data and fetched subscription content must remain separate.
- Every write path must validate or stage before commit and must support rollback.
- Every background task must be single-flight where appropriate, bounded, cancellable, and observable.
- No in-memory map, history directory, log stream, or retry loop may grow without a cap.
- Existing profiles, rule files, backups, and `verge.yaml` values must keep working.
- All code, comments, commits, and PR text are in English.

## Child tasks

| Child | Deliverable | Dependency |
|---|---|---|
| `10-20-custom-rules-quick-add` | `sync_id`, `custom-rules.yaml`, enhancement injection, quick-add UI, subscription-update compatibility | None |
| `10-20-subscription-node-health` | Settings section, Rust health manager, expected-status probes, node filters | None; settings schema should be frozen before sync |
| `10-20-webdav-sync` | Manifest/state, scheduled sync, record-level conflicts, encryption, apply/rollback | Requires child 1 `sync_id` and child 2 settings schema |

## Cross-child acceptance criteria

- [ ] Two profiles with different proxy groups can each add a custom rule without affecting the other.
- [ ] A subscription update that removes a referenced group never silently writes a broken profile; the user can remap, change to DIRECT, delete, or cancel.
- [ ] A node can be marked `unusable`, `site_blocked`, or `degraded` based on automated probes, not only on a latency number.
- [ ] A user can switch a node and immediately see whether it passes a real-site check.
- [ ] A node visibility filter can be set globally and overridden per group from the group toolbar.
- [ ] Two devices can sync subscription metadata, overrides, and custom rules without uploading fetched proxy content.
- [ ] A sync conflict with last-modified-wins retains the losing copy and never deletes local data without a recoverable backup.
- [ ] Existing profiles, old `r<uid>.yaml` rules, old backups, and old `enable_auto_delay_detection` settings continue to work after migration.
- [ ] A 24-hour soak with background health checks and sync enabled shows bounded memory, handles, and log growth.

## Constraints

- Follow repository `AGENTS.md`, `src/AGENTS.md`, and `src-tauri/AGENTS.md`.
- Issue-first; each PR must map to an issue and one child task scope.
- Reuse the existing configuration draft/lock model, enhancement pipeline, validation, WebDAV client, scheduler patterns, and generated command bindings.
- Do not auto-restart the application after a scheduled sync.
- Do not change mihomo group filters for display-only hiding.
- Do not add tests or defensive scaffolding unless the failure would otherwise be silent or data-destructive.
