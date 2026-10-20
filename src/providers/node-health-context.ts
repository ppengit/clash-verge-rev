import { createContext, use } from 'react'

import type {
  NodeHealthEntry,
  NodeHealthSnapshot,
  NodeHealthState,
} from '@/services/bindings'

export interface NodeHealthContextType {
  snapshot: NodeHealthSnapshot | undefined
  entries: ReadonlyMap<string, NodeHealthEntry>
  stateOf: (group: string, node: string) => NodeHealthState | undefined
  entryOf: (group: string, node: string) => NodeHealthEntry | undefined
  refresh: () => Promise<unknown>
  isLoading: boolean
}

export const NodeHealthContext = createContext<NodeHealthContextType | null>(
  null,
)

export const useNodeHealth = (): NodeHealthContextType => {
  const value = use(NodeHealthContext)
  if (!value) {
    throw new Error('useNodeHealth must be used within NodeHealthProvider')
  }
  return value
}

export const useNodeHealthEntry = (group: string, node: string) => {
  const { entryOf } = useNodeHealth()
  return entryOf(group, node)
}
