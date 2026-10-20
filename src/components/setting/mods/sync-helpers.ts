import dayjs from 'dayjs'

export const SYNC_SCOPES = [
  'subscriptions',
  'overrides',
  'custom_rules',
  'settings',
] as const

export type SyncScope = (typeof SYNC_SCOPES)[number]

export const DEFAULT_SYNC_SCOPES: SyncScope[] = [
  'subscriptions',
  'overrides',
  'custom_rules',
]

export const SCOPE_LABEL_KEYS = {
  subscriptions: 'settings.modals.sync.options.scopes.subscriptions',
  overrides: 'settings.modals.sync.options.scopes.overrides',
  custom_rules: 'settings.modals.sync.options.scopes.customRules',
  settings: 'settings.modals.sync.options.scopes.settings',
} as const

export const SCOPE_HELPER_KEYS = {
  subscriptions: 'settings.modals.sync.helpers.scopeSubscriptions',
  overrides: 'settings.modals.sync.helpers.scopeOverrides',
  custom_rules: 'settings.modals.sync.helpers.scopeCustomRules',
  settings: 'settings.modals.sync.helpers.scopeSettings',
} as const

const isSyncScope = (value: string): value is SyncScope =>
  (SYNC_SCOPES as readonly string[]).includes(value)

export const orderedScopes = (scopes: readonly string[]): SyncScope[] =>
  SYNC_SCOPES.filter((scope) => scopes.includes(scope))

export const formatSyncTime = (value?: string | null) => {
  if (!value) return null
  const time = dayjs(value)
  return time.isValid() ? time.format('YYYY-MM-DD HH:mm') : value
}

export const formatSyncTimestamp = (value?: number | null) => {
  if (!value) return null
  const time = dayjs(value)
  return time.isValid() ? time.format('YYYY-MM-DD HH:mm') : null
}

export const formatSyncBytes = (bytes?: number | null) => {
  if (!bytes || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  const index = Math.min(
    units.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024)),
  )
  const value = bytes / 1024 ** index
  return `${index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`
}

/** Missing scopes from the backend upload guard, e.g. "not enabled on this device (overrides)". */
export const missingScopesFromError = (
  message?: string | null,
): SyncScope[] => {
  if (!message) return []
  const match = message.match(/not enabled on this device \(([^)]+)\)/i)
  if (!match) return []
  return match[1]
    .split(',')
    .map((name) => name.trim())
    .filter(isSyncScope)
}
