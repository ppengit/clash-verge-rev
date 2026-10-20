import {
  Alert,
  Autocomplete,
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  MenuItem,
  Select,
  TextField,
  Typography,
} from '@mui/material'
import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { fetchProfilesIntoCache } from '@/hooks/use-profiles'
import { useAppRefreshers } from '@/providers/app-data-context'
import type { ConflictAction, RuleConflict } from '@/services/bindings'
import {
  cancelProfileUpdateConflicts,
  resolveProfileUpdateConflicts,
} from '@/services/cmds'
import { showNotice } from '@/services/notice-service'
import {
  advanceProfileUpdateConflict,
  subscribeProfileUpdateConflicts,
  type ProfileUpdateConflictEvent,
} from '@/services/profile-update-conflict'

type ChoiceAction = ConflictAction | 'keep'

interface Choice {
  action: ChoiceAction
  policy: string
}

const conflictKey = (conflict: RuleConflict) =>
  conflict.ruleId ?? `legacy:${conflict.position ?? ''}:${conflict.rule}`

export const ProfileUpdateConflictDialog = () => {
  const { t } = useTranslation()
  const { refreshAll } = useAppRefreshers()
  const [event, setEvent] = useState<ProfileUpdateConflictEvent | null>(null)
  const [choices, setChoices] = useState<Record<string, Choice>>({})
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(
    () =>
      subscribeProfileUpdateConflicts((next) => {
        setEvent(next)
        setError(null)
        setChoices(
          next
            ? Object.fromEntries(
                next.conflicts.map((conflict) => [
                  conflictKey(conflict),
                  { action: 'keep', policy: '' },
                ]),
              )
            : {},
        )
      }),
    [],
  )

  const policies = useMemo(
    () =>
      event
        ? [...event.targets.builtinPolicies, ...event.targets.groups].filter(
            (policy, index, all) => all.indexOf(policy) === index,
          )
        : [],
    [event],
  )

  const updateChoice = (key: string, patch: Partial<Choice>) => {
    setChoices((current) => ({
      ...current,
      [key]: { ...current[key], ...patch },
    }))
  }

  const onCancel = async () => {
    if (!event) return
    try {
      await cancelProfileUpdateConflicts(event.profileUid)
    } catch (err) {
      showNotice.error(err)
    } finally {
      setError(null)
      advanceProfileUpdateConflict()
    }
  }

  const onSubmit = async () => {
    if (!event) return
    const unresolved = event.conflicts.filter(
      (conflict) =>
        (choices[conflictKey(conflict)]?.action ?? 'keep') === 'keep',
    )
    if (unresolved.length > 0) {
      setError(t('profiles.modals.updateConflict.errors.unresolved'))
      return
    }

    const resolutions = event.conflicts.map((conflict) => {
      const choice = choices[conflictKey(conflict)] ?? {
        action: 'keep' as const,
        policy: '',
      }
      return {
        ruleId: conflict.ruleId ?? null,
        rule: conflict.rule,
        action: choice.action as ConflictAction,
        policy: choice.action === 'remap' ? choice.policy : null,
      }
    })

    setSubmitting(true)
    setError(null)
    try {
      const report = await resolveProfileUpdateConflicts(
        event.profileUid,
        resolutions,
      )
      if (report.status === 'conflict') {
        setEvent({
          profileUid: event.profileUid,
          conflicts: report.conflicts,
          targets: report.targets,
        })
        setChoices(
          Object.fromEntries(
            report.conflicts.map((conflict) => [
              conflictKey(conflict),
              { action: 'keep', policy: '' },
            ]),
          ),
        )
        showNotice.warning('profiles.modals.updateConflict.errors.remaining')
        return
      }
      showNotice.success('profiles.modals.updateConflict.feedback.updated')
      advanceProfileUpdateConflict()
      await Promise.all([refreshAll(), fetchProfilesIntoCache()])
    } catch (err) {
      setError(String(err))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Dialog
      open={Boolean(event)}
      onClose={() => {
        if (!submitting) void onCancel()
      }}
      maxWidth="md"
      fullWidth
    >
      <DialogTitle>{t('profiles.modals.updateConflict.title')}</DialogTitle>
      <DialogContent sx={{ display: 'grid', gap: 2 }}>
        <Typography color="text.secondary" variant="body2">
          {t('profiles.modals.updateConflict.description')}
        </Typography>
        {error && <Alert severity="error">{error}</Alert>}
        {event?.conflicts.map((conflict) => {
          const key = conflictKey(conflict)
          const choice = choices[key] ?? { action: 'keep', policy: '' }
          const policyEditable = conflict.code === 'policy'
          return (
            <Box
              key={key}
              sx={{
                display: 'grid',
                gap: 1,
                p: 1.5,
                border: '1px solid var(--divider-color)',
                borderRadius: 1,
              }}
            >
              <Typography variant="body2" sx={{ wordBreak: 'break-all' }}>
                {conflict.rule}
              </Typography>
              <Typography variant="caption" color="text.secondary">
                {conflict.missing}
              </Typography>
              {!policyEditable && (
                <Typography variant="caption" color="warning.main">
                  {t('profiles.modals.updateConflict.errors.unresolvable')}
                </Typography>
              )}
              <Box sx={{ display: 'flex', gap: 1, flexWrap: 'wrap' }}>
                <Select
                  size="small"
                  value={choice.action}
                  onChange={(changeEvent) =>
                    updateChoice(key, {
                      action: changeEvent.target.value as ChoiceAction,
                    })
                  }
                  sx={{ minWidth: 160 }}
                >
                  <MenuItem value="keep">
                    {t('profiles.modals.updateConflict.actions.keep')}
                  </MenuItem>
                  {policyEditable && (
                    <MenuItem value="remap">
                      {t('profiles.modals.updateConflict.actions.remap')}
                    </MenuItem>
                  )}
                  {policyEditable && <MenuItem value="direct">DIRECT</MenuItem>}
                  {policyEditable && <MenuItem value="reject">REJECT</MenuItem>}
                  <MenuItem value="delete">
                    {t('profiles.modals.updateConflict.actions.delete')}
                  </MenuItem>
                </Select>
                {policyEditable && choice.action === 'remap' && (
                  <Autocomplete
                    size="small"
                    options={policies}
                    value={choice.policy || null}
                    onChange={(_, value) =>
                      updateChoice(key, { policy: value ?? '' })
                    }
                    renderInput={(params) => (
                      <TextField
                        {...params}
                        placeholder={t(
                          'profiles.modals.updateConflict.fields.policy',
                        )}
                      />
                    )}
                    sx={{ minWidth: 220 }}
                  />
                )}
              </Box>
            </Box>
          )
        })}
      </DialogContent>
      <DialogActions>
        <Button variant="outlined" onClick={onCancel} disabled={submitting}>
          {t('shared.actions.cancel')}
        </Button>
        <Button
          variant="contained"
          onClick={onSubmit}
          disabled={submitting || !event}
        >
          {t('profiles.modals.updateConflict.actions.apply')}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
