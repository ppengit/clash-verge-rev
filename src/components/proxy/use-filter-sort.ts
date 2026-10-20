import type { NodeHealthState } from '@/services/bindings'
import delayManager from '@/services/delay'
import { isUnhealthyNode, nodeHealthSortRank } from '@/services/node-health'
import { memberDetails, type ResolvedProxyMember } from '@/types/proxy-view'
import {
  classifyDelay,
  compareByDelay,
  DEFAULT_DELAY_TIMEOUT,
} from '@/utils/delay'
import { compileStringMatcher } from '@/utils/search-matcher'

import type { ResolvedMemberOccurrence } from './use-render-list'

export type ProxySortType = 0 | 1 | 2

export type ProxySearchState = {
  matchCase?: boolean
  matchWholeWord?: boolean
  useRegularExpression?: boolean
}

export interface ProxyDisplayFilter {
  /** Hide timeout and error delays. */
  hideTimeout?: boolean
  hideUntested?: boolean
  /** Hide health states that cannot carry traffic. */
  hideUnhealthy?: boolean
  /** Hide measured delays above this value; zero disables the filter. */
  hideSlowMs?: number
  /** The group's current node stays visible even when it matches a hide rule. */
  pinnedNode?: string | null
  /** Per-group "show all" override. */
  bypass?: boolean
}

export interface FilterSortOptions {
  healthState?: (member: ResolvedProxyMember) => NodeHealthState | undefined
  display?: ProxyDisplayFilter
}

export function filterSort(
  proxies: ResolvedMemberOccurrence[],
  groupName: string,
  filterText: string,
  sortType: ProxySortType,
  latencyTimeout?: number,
  searchState?: ProxySearchState,
  options?: FilterSortOptions,
) {
  const fp = filterProxies(
    proxies,
    groupName,
    filterText,
    searchState,
    options,
    latencyTimeout,
  )
  const sp = sortProxies(
    fp,
    groupName,
    sortType,
    latencyTimeout,
    options?.healthState,
  )
  return sp
}

const regex1 = /delay([=<>])(\d+|timeout|error)/i
const regex2 = /type=(.*)/i

const displayFiltersActive = (display: ProxyDisplayFilter) =>
  display.hideTimeout ||
  display.hideUntested ||
  display.hideUnhealthy ||
  (typeof display.hideSlowMs === 'number' && display.hideSlowMs > 0)

const isHiddenByDisplay = (
  member: ResolvedProxyMember,
  groupName: string,
  display: ProxyDisplayFilter,
  healthState?: (member: ResolvedProxyMember) => NodeHealthState | undefined,
  latencyTimeout?: number,
) => {
  if (display.pinnedNode && member.ref.name === display.pinnedNode) return false
  if (member.kind === 'unresolved') return false

  const health = healthState?.(member)
  if (display.hideUnhealthy && isUnhealthyNode(health)) return true
  if (!display.hideTimeout && !display.hideUntested && !display.hideSlowMs) {
    return false
  }

  const delay = delayManager.getDelayFix(member, groupName)
  const timeout =
    typeof latencyTimeout === 'number' && latencyTimeout > 0
      ? latencyTimeout
      : DEFAULT_DELAY_TIMEOUT
  const delayState = classifyDelay(delay, timeout)
  if (
    display.hideTimeout &&
    (delayState === 'timeout' || delayState === 'error')
  ) {
    return true
  }
  if (
    display.hideUntested &&
    delayState === 'untested' &&
    (health === undefined || health === 'unverified')
  ) {
    return true
  }
  if (
    display.hideSlowMs &&
    display.hideSlowMs > 0 &&
    delayState === 'measured' &&
    delay > display.hideSlowMs
  ) {
    return true
  }
  return false
}

function filterProxies(
  proxies: ResolvedMemberOccurrence[],
  groupName: string,
  filterText: string,
  searchState?: ProxySearchState,
  options?: FilterSortOptions,
  latencyTimeout?: number,
) {
  const query = filterText.trim()
  if (!query) {
    const display = options?.display
    if (!display || display.bypass || !displayFiltersActive(display)) {
      return proxies
    }
    return proxies.filter(
      ({ member }) =>
        !isHiddenByDisplay(
          member,
          groupName,
          display,
          options?.healthState,
          latencyTimeout,
        ),
    )
  }

  // Search always reaches hidden nodes.
  const res1 = regex1.exec(query)
  if (res1) {
    const symbol = res1[1]
    const symbol2 = res1[2].toLowerCase()
    const value =
      symbol2 === 'error' ? 1e5 : symbol2 === 'timeout' ? 3000 : +symbol2

    return proxies.filter(({ member }) => {
      const delay = delayManager.getDelayFix(member, groupName)

      if (delay < 0) return false
      if (symbol === '=' && symbol2 === 'error') return delay >= 1e5
      if (symbol === '=' && symbol2 === 'timeout')
        return delay < 1e5 && delay >= 3000
      if (symbol === '=') return delay == value
      if (symbol === '<') return delay <= value
      if (symbol === '>') return delay >= value
      return false
    })
  }

  const res2 = regex2.exec(query)
  if (res2) {
    const type = res2[1].toLowerCase()
    return proxies.filter(({ member }) =>
      (memberDetails(member)?.type ?? '').toLowerCase().includes(type),
    )
  }

  const {
    matchCase = false,
    matchWholeWord = false,
    useRegularExpression = false,
  } = searchState ?? {}
  const compiled = compileStringMatcher(query, {
    matchCase,
    matchWholeWord,
    useRegularExpression,
  })

  if (!compiled.isValid) return []
  return proxies.filter(({ member }) => compiled.matcher(member.ref.name))
}

function sortProxies(
  proxies: ResolvedMemberOccurrence[],
  groupName: string,
  sortType: ProxySortType,
  latencyTimeout?: number,
  healthState?: (member: ResolvedProxyMember) => NodeHealthState | undefined,
) {
  if (!proxies) return []
  if (sortType === 0) return proxies

  const effectiveTimeout =
    typeof latencyTimeout === 'number' && latencyTimeout > 0
      ? latencyTimeout
      : DEFAULT_DELAY_TIMEOUT

  if (sortType === 1 && proxies.length > 1) {
    return proxies
      .map((proxy) => ({
        proxy,
        delay: delayManager.getDelayFix(proxy.member, groupName),
        health: nodeHealthSortRank(healthState?.(proxy.member)),
      }))
      .sort(
        (a, b) =>
          a.health - b.health ||
          compareByDelay(a.delay, b.delay, effectiveTimeout),
      )
      .map(({ proxy }) => proxy)
  }
  const list = proxies.slice()
  if (sortType !== 1) {
    list.sort((a, b) => a.member.ref.name.localeCompare(b.member.ref.name))
  }

  return list
}
