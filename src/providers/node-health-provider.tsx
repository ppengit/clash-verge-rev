import { useCallback, useMemo, useRef, type ReactNode } from 'react'

import { getNodeHealth } from '@/services/cmds'
import { buildNodeHealthMap, nodeHealthKey } from '@/services/node-health'
import { useQuery } from '@/services/query-client'

import {
  NodeHealthContext,
  type NodeHealthContextType,
} from './node-health-context'

/**
 * The single owner of the node health query. The event bus revalidates it on
 * `verge://node-health-updated`; components read the derived map through
 * `useNodeHealth` instead of subscribing themselves.
 */
export const NodeHealthProvider = ({ children }: { children: ReactNode }) => {
  const {
    data: snapshot,
    refetch,
    isLoading,
  } = useQuery({
    queryKey: ['getNodeHealth'],
    queryFn: getNodeHealth,
    staleTime: 5000,
    retry: 1,
  })

  const entries = useMemo(
    () => buildNodeHealthMap(snapshot?.entries ?? []),
    [snapshot],
  )

  const entryOf = useCallback(
    (group: string, node: string) => entries.get(nodeHealthKey(group, node)),
    [entries],
  )

  const stateOf = useCallback(
    (group: string, node: string) => entryOf(group, node)?.state,
    [entryOf],
  )

  const refetchRef = useRef(refetch)
  refetchRef.current = refetch
  const refresh = useCallback(() => refetchRef.current(), [])

  const value = useMemo<NodeHealthContextType>(
    () => ({ snapshot, entries, stateOf, entryOf, refresh, isLoading }),
    [snapshot, entries, stateOf, entryOf, refresh, isLoading],
  )

  return <NodeHealthContext value={value}>{children}</NodeHealthContext>
}
