# Standalone custom rules and quick rule creation

## Goal

Move user-created rules out of the fetched subscription profile and into a standalone `custom-rules.yaml` keyed by a stable profile `sync_id`, then make rule creation a first-class action from the Rules and Connections pages.

## User stories

- As a user, I can click "Add rule" on the Rules page, enter a domain, choose a proxy group, and see the rule take effect immediately.
- As a user, I can right-click a connection and add a rule for its domain, process, IP, or port with the value prefilled.
- As a user with multiple subscriptions, I can see which custom rules belong to which subscription and understand that switching subscriptions switches the rule set.
- As a user, when a subscription update removes a proxy group referenced by my rule, I am asked to remap, change to DIRECT/REJECT, delete, or cancel instead of getting a broken config.
- As a user, I can undo a rule I just added.
- As a user, I can see whether a rule is from the subscription, the legacy custom file, the new custom file, or the global config.

## Requirements

### Identity

- Add optional `PrfItem.sync_id`, generated with `help::get_uid("S")`.
- Lazily generate and persist `sync_id` for existing profiles.
- Custom rules reference `sync_id`, not local `uid`.
- Missing or failed `sync_id` persistence must not block startup.

### Storage

- New file: `app_home_dir/custom-rules.yaml`, schema version 1.
- Rule fields: `id`, `profile` (`sync_id` or `*`), `position` (`prepend`/`append`), `type`, `value`, `policy`, `policy_kind`, `no_resolve`, `enabled`, `created_at`, `updated_at`, `deleted`.
- Limits: 2 MB file, 10,000 rules, 2,048-character value.
- Atomic write with a previous-file backup; serialize writes with a dedicated lock.
- Load with `mtime + size` cache; malformed entries are skipped with a rate-limited warning.

### Enhancement

- Inject after Script and before final cleanup.
- Prepend rules go to the top.
- Append rules go before the first `MATCH`; if no `MATCH`, append at the end.
- Skip rules whose policy, rule-provider, or sub-rule no longer exists; do not fail the whole config.
- Keep legacy `r<uid>.yaml` rules working.
- Do not allow quick-add to create `MATCH` rules; advanced YAML editing may.

### Quick add

- Entry points: Rules page header button, Rules page empty state, Connections row/detail context menu.
- Dialog fields: target profile, rule type, value, policy, position, `no-resolve`, preview.
- Policy list:
  - Current profile: runtime proxy view, including Merge/Script results.
  - Non-current profile: parse that profile, its Merge, and global Merge; allow manual group input if a Script can generate groups dynamically.
  - Built-in policies always available.
- Duplicate detection: exact duplicate in the same profile and position is blocked; a duplicate in subscription rules shows an override hint.
- Save immediately for the current profile; non-current profiles save without applying until selected.
- Undo toast after add.
- "Apply to all subscriptions" supports built-in policies only in v1; group policies require per-subscription mapping later.

### Subscription update compatibility

- Download new subscription to a staging area; parse groups, rule-providers, and sub-rules before replacing the profile.
- Validate new custom rules and legacy rules against the staged config.
- On conflict, return a structured error; the frontend shows a remap dialog.
- Timer-triggered updates are non-interactive: keep the old profile, notify, and retry at the normal interval.
- Never write a profile that leaves the config invalid.

### Rules page

- Show source badges: subscription, legacy custom, new custom, global.
- Filter: all / custom / subscription.
- Custom rule row menu: copy, delete, rebind policy.
- Search empty state offers "Add rule".

## Acceptance criteria

- [ ] From the Rules page, a user can add a valid rule in three interactions or fewer and see it in the runtime rules list without restarting.
- [ ] From a connection, domain/IP/process/port suggestions prefill the dialog correctly.
- [ ] A rule added under profile A does not appear under profile B.
- [ ] A rule for a non-current profile is saved but not applied until that profile is selected.
- [ ] Invalid syntax, unknown policy, duplicate rule, missing profile, and oversized input are rejected without changing the file.
- [ ] If enhancing the current profile fails after a rule add, the previous `custom-rules.yaml` is restored and the error is shown.
- [ ] Undo removes the just-added rule and re-applies the current profile.
- [ ] A subscription update that removes a referenced group prompts for remap/change/delete/cancel; cancel leaves the old profile intact.
- [ ] Legacy `r<uid>.yaml` rules still apply and are labeled as legacy custom.
- [ ] Malformed entries in `custom-rules.yaml` do not prevent startup.
- [ ] A 10,000-rule file loads within the stated performance budget.

## Non-goals

- Rule hit counters or traffic statistics.
- Automatic migration of legacy rules without user confirmation.
- Global group-policy rules in v1.
- Editing subscription-provided rules in place; custom rules only prepend/append.
- A full rules YAML formatter that preserves comments (document the limitation).

## Constraints

- Follow `AGENTS.md`, `src/AGENTS.md`, and `src-tauri/AGENTS.md`.
- Reuse `save_profile_file` validation/apply patterns.
- No unbounded arrays, caches, or retry loops.
- Keep code, comments, commits, and PR text in English.
