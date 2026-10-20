import type { TranslationKey } from '@/types/generated/i18n-keys'

import type { NodeHealthEntry, NodeHealthState } from './bindings'

const KEY_SEPARATOR = '\u0000'

/** Stable identity for the health map: the backend keys entries by group and node. */
export const nodeHealthKey = (group: string, node: string) =>
  `${group}${KEY_SEPARATOR}${node}`

export const buildNodeHealthMap = (entries: readonly NodeHealthEntry[]) => {
  const map = new Map<string, NodeHealthEntry>()
  for (const entry of entries) {
    map.set(nodeHealthKey(entry.key.group, entry.key.node), entry)
  }
  return map
}

const HEALTH_RANKS: Record<NodeHealthState, number> = {
  healthy: 0,
  testing: 1,
  degraded: 2,
  unverified: 3,
  site_blocked: 4,
  unusable: 5,
}

/** Unknown health sorts with unverified so nodes without data keep their latency order. */
export const nodeHealthSortRank = (state?: NodeHealthState) =>
  state === undefined ? HEALTH_RANKS.unverified : HEALTH_RANKS[state]

export const NODE_HEALTH_STATE_LABEL_KEYS: Record<
  NodeHealthState,
  TranslationKey
> = {
  healthy: 'proxies.page.health.states.healthy',
  degraded: 'proxies.page.health.states.degraded',
  site_blocked: 'proxies.page.health.states.siteBlocked',
  unusable: 'proxies.page.health.states.unusable',
  testing: 'proxies.page.health.states.testing',
  unverified: 'proxies.page.health.states.unverified',
}

export const isUnhealthyNode = (state?: NodeHealthState) =>
  state === 'unusable' || state === 'site_blocked'

/** States that make a selection warning worthwhile. */
export const isBadNodeState = (state?: NodeHealthState) =>
  state === 'unusable' || state === 'site_blocked'
