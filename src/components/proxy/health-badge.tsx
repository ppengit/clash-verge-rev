import { Box, Tooltip } from '@mui/material'
import { useTranslation } from 'react-i18next'

import { useNodeHealthEntry } from '@/providers/node-health-context'
import type { NodeHealthState } from '@/services/bindings'
import { NODE_HEALTH_STATE_LABEL_KEYS } from '@/services/node-health'

const STATE_COLORS: Record<NodeHealthState, string> = {
  healthy: 'success.main',
  degraded: 'warning.main',
  site_blocked: 'error.light',
  unusable: 'error.main',
  testing: 'info.main',
  unverified: 'text.disabled',
}

interface Props {
  group: string
  node: string
}

/** Compact health dot; the tooltip carries the detail so the row stays quiet. */
export const HealthBadge = ({ group, node }: Props) => {
  const { t } = useTranslation()
  const entry = useNodeHealthEntry(group, node)
  if (!entry) return null

  const label = t(NODE_HEALTH_STATE_LABEL_KEYS[entry.state])
  const failed =
    entry.failedTargets.length > 0
      ? `\n${t('proxies.page.health.failedTargets', {
          targets: entry.failedTargets.join(', '),
        })}`
      : ''
  const delay =
    entry.delayMs === null || entry.delayMs === undefined
      ? ''
      : `\n${t('proxies.page.health.delay', { delay: entry.delayMs })}`

  return (
    <Tooltip title={`${label}${delay}${failed}`} arrow>
      <Box
        component="span"
        role="img"
        aria-label={label}
        sx={{
          display: 'inline-block',
          flex: '0 0 auto',
          width: 8,
          height: 8,
          ml: 0.75,
          borderRadius: '50%',
          bgcolor: STATE_COLORS[entry.state],
          verticalAlign: 'middle',
        }}
      />
    </Tooltip>
  )
}
