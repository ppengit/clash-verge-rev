import type { ProfileUpdateReport, RuleConflict, RuleTargets } from './bindings'

export interface ProfileUpdateConflictEvent {
  profileUid: string
  conflicts: RuleConflict[]
  targets: RuleTargets
}

type Listener = (event: ProfileUpdateConflictEvent | null) => void

// A batch update can produce one conflict report per subscription. Keep one
// active report and a bounded FIFO so earlier reports are not silently dropped.
const MAX_QUEUED_CONFLICTS = 32
const listeners = new Set<Listener>()
const queue: ProfileUpdateConflictEvent[] = []
let active: ProfileUpdateConflictEvent | null = null

const emit = () => {
  listeners.forEach((listener) => listener(active))
}

export const reportProfileUpdateConflict = (
  profileUid: string,
  conflicts: RuleConflict[],
  targets: RuleTargets,
) => {
  const event: ProfileUpdateConflictEvent = { profileUid, conflicts, targets }

  if (active?.profileUid === profileUid) {
    active = event
    emit()
    return
  }

  const queued = queue.findIndex((item) => item.profileUid === profileUid)
  if (queued >= 0) {
    queue[queued] = event
    return
  }

  if (active) {
    if (queue.length >= MAX_QUEUED_CONFLICTS) queue.shift()
    queue.push(event)
    return
  }

  active = event
  emit()
}

export const subscribeProfileUpdateConflicts = (listener: Listener) => {
  listeners.add(listener)
  listener(active)
  return () => {
    listeners.delete(listener)
  }
}

/** Close the current report and surface the next queued subscription. */
export const advanceProfileUpdateConflict = () => {
  active = queue.shift() ?? null
  emit()
}

/**
 * Surfaces a remap dialog for a manual update. Timer updates never reach the
 * frontend: the backend keeps the old profile and notifies on its own.
 */
export const handleProfileUpdateReport = (
  profileUid: string,
  report: ProfileUpdateReport,
) => {
  if (report.status === 'conflict') {
    reportProfileUpdateConflict(profileUid, report.conflicts, report.targets)
  }
  return report
}
