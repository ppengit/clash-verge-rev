import { RestoreRounded, RefreshRounded } from '@mui/icons-material'
import {
  Box,
  Button,
  IconButton,
  List,
  ListItem,
  ListItemText,
  Stack,
  Typography,
} from '@mui/material'
import { useLockFn } from 'ahooks'
import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { BaseDialog, BaseLoadingOverlay } from '@/components/base'
import { listSyncHistory, restoreSyncHistory } from '@/services/cmds'
import type { SyncHistoryEntry } from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { errorDetail, showNotice } from '@/services/notice-service'

import { formatSyncBytes, formatSyncTimestamp } from './sync-helpers'

interface SyncHistoryViewerProps {
  open: boolean
  disabled: boolean
  onClose: () => void
  onRestored: () => Promise<void>
}

export const SyncHistoryViewer = ({
  open,
  disabled,
  onClose,
  onRestored,
}: SyncHistoryViewerProps) => {
  const { t } = useTranslation()
  const [rows, setRows] = useState<SyncHistoryEntry[]>([])
  const [loading, setLoading] = useState(false)
  const [restoring, setRestoring] = useState<string | null>(null)
  const [pending, setPending] = useState<SyncHistoryEntry | null>(null)

  const fetchRows = useCallback(async () => {
    if (!open) return
    setLoading(true)
    try {
      const outcome = await mutate(() => listSyncHistory(), {
        id: 'list-sync-history',
        errorNotice: false,
      })
      if (outcome.ok) {
        setRows([...outcome.value].sort((a, b) => b.revision - a.revision))
      }
    } catch (error) {
      setRows([])
      showNotice.error('settings.modals.sync.messages.historyFailed', {
        error: errorDetail(error),
      })
    } finally {
      setLoading(false)
    }
  }, [open])

  useEffect(() => {
    void fetchRows()
  }, [fetchRows])

  const confirmRestore = useLockFn(async () => {
    if (!pending) return
    setRestoring(pending.filename)
    try {
      const outcome = await mutate(() => restoreSyncHistory(pending.filename), {
        id: 'restore-sync-history',
        errorNotice: false,
      })
      if (!outcome.ok) return
      setPending(null)
      showNotice.success('settings.modals.sync.messages.historyRestored')
      await onRestored()
      await fetchRows()
    } catch (error) {
      showNotice.error('settings.modals.sync.messages.restoreFailed', {
        error: errorDetail(error),
      })
    } finally {
      setRestoring(null)
    }
  })

  const busy = loading || restoring !== null || disabled

  return (
    <BaseDialog
      open={open}
      title={t('settings.modals.sync.history.title')}
      maxWidth="sm"
      fullWidth
      dividers
      contentSx={{ maxHeight: '70vh', scrollbarGutter: 'stable' }}
      disableOk
      cancelBtn={t('shared.actions.close')}
      onCancel={onClose}
      onClose={onClose}
    >
      <Box sx={{ position: 'relative', minHeight: 240 }}>
        <BaseLoadingOverlay isLoading={loading || restoring !== null} />
        <Stack spacing={1.5}>
          <Stack
            direction="row"
            spacing={1}
            sx={{ alignItems: 'center', justifyContent: 'space-between' }}
          >
            <Typography variant="caption" color="text.secondary">
              {t('settings.modals.sync.history.helper')}
            </Typography>
            <IconButton
              size="small"
              disabled={busy}
              onClick={() => void fetchRows()}
            >
              <RefreshRounded fontSize="small" />
            </IconButton>
          </Stack>

          {rows.length === 0 ? (
            <Typography variant="body2" color="text.secondary">
              {t('settings.modals.sync.history.empty')}
            </Typography>
          ) : (
            <List disablePadding>
              {rows.map((row, index) => {
                const time = formatSyncTimestamp(row.createdAt)
                const device = row.deviceName || row.deviceId
                return (
                  <ListItem
                    key={`${row.revision}-${row.filename}`}
                    divider={index < rows.length - 1}
                    disableGutters
                    secondaryAction={
                      <Button
                        size="small"
                        variant="outlined"
                        startIcon={<RestoreRounded fontSize="small" />}
                        disabled={busy}
                        loading={restoring === row.filename}
                        onClick={() => setPending(row)}
                      >
                        {t('settings.modals.sync.actions.restore')}
                      </Button>
                    }
                  >
                    <ListItemText
                      primary={t('settings.modals.sync.history.revision', {
                        revision: row.revision,
                      })}
                      secondary={
                        <Stack
                          component="span"
                          direction="row"
                          spacing={1}
                          useFlexGap
                          sx={{ flexWrap: 'wrap' }}
                        >
                          <Typography
                            component="span"
                            variant="caption"
                            color="text.secondary"
                          >
                            {device
                              ? t('settings.modals.sync.history.device', {
                                  device,
                                })
                              : t('settings.modals.sync.history.unknownDevice')}
                          </Typography>
                          <Typography
                            component="span"
                            variant="caption"
                            color="text.secondary"
                          >
                            {time ?? row.filename}
                          </Typography>
                          <Typography
                            component="span"
                            variant="caption"
                            color="text.secondary"
                          >
                            {formatSyncBytes(row.payloadSize)}
                          </Typography>
                        </Stack>
                      }
                      slotProps={{ secondary: { component: 'span' } }}
                    />
                  </ListItem>
                )
              })}
            </List>
          )}
        </Stack>
      </Box>

      <BaseDialog
        open={pending !== null}
        title={t('settings.modals.sync.confirm.restoreTitle')}
        okBtn={t('shared.actions.confirm')}
        cancelBtn={t('shared.actions.cancel')}
        contentSx={{ width: { xs: 320, sm: 440 } }}
        loading={restoring !== null}
        onCancel={() => {
          if (restoring === null) setPending(null)
        }}
        onClose={() => {
          if (restoring === null) setPending(null)
        }}
        onOk={() => void confirmRestore()}
      >
        <Typography variant="body2" sx={{ wordBreak: 'break-word' }}>
          {t('settings.modals.sync.confirm.restore', {
            revision: pending?.revision ?? 0,
          })}
        </Typography>
      </BaseDialog>
    </BaseDialog>
  )
}
