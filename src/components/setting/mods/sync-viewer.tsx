import {
  Alert,
  Button,
  Checkbox,
  FormControlLabel,
  MenuItem,
  Stack,
  TextField,
  Typography,
} from '@mui/material'
import { useLockFn } from 'ahooks'
import type { ChangeEvent, ReactNode, Ref } from 'react'
import { useImperativeHandle, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { BaseDialog, Switch } from '@/components/base'
import type { DialogRef } from '@/components/base'
import { useVerge } from '@/hooks/use-verge'
import { getSyncStatus, syncNow, testWebdavSync } from '@/services/cmds'
import type { SyncSettings } from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { errorDetail, showNotice } from '@/services/notice-service'
import { revalidateQuery, useQuery } from '@/services/query-client'

import {
  DEFAULT_SYNC_SCOPES,
  SCOPE_HELPER_KEYS,
  SCOPE_LABEL_KEYS,
  SYNC_SCOPES,
  formatSyncTime,
  formatSyncTimestamp,
  missingScopesFromError,
  orderedScopes,
} from './sync-helpers'
import type { SyncScope } from './sync-helpers'
import { SyncHistoryViewer } from './sync-history-viewer'

type SyncBusy = 'test' | 'sync' | null
type FirstSyncChoice = 'upload' | 'download'

const MIN_INTERVAL_MINUTES = 15
const MAX_DEVICE_NAME_LENGTH = 32

const Section = ({
  title,
  children,
}: {
  title: string
  children: ReactNode
}) => (
  <Stack
    spacing={1.25}
    sx={{
      border: (theme) => `1px solid ${theme.palette.divider}`,
      borderRadius: 2,
      p: 2,
    }}
  >
    <Typography variant="subtitle2">{title}</Typography>
    {children}
  </Stack>
)

const StatusRow = ({ label, value }: { label: string; value: ReactNode }) => (
  <Stack
    direction="row"
    spacing={2}
    sx={{ alignItems: 'flex-start', justifyContent: 'space-between' }}
  >
    <Typography variant="body2" color="text.secondary">
      {label}
    </Typography>
    <Typography
      variant="body2"
      sx={{ textAlign: 'right', wordBreak: 'break-word' }}
    >
      {value}
    </Typography>
  </Stack>
)

const SwitchRow = ({
  label,
  helper,
  checked,
  disabled,
  onChange,
}: {
  label: string
  helper?: string
  checked: boolean
  disabled: boolean
  onChange: (event: ChangeEvent<HTMLInputElement>, checked: boolean) => void
}) => (
  <Stack
    direction="row"
    spacing={2}
    sx={{ alignItems: 'center', justifyContent: 'space-between' }}
  >
    <Stack>
      <Typography variant="body2">{label}</Typography>
      {helper ? (
        <Typography variant="caption" color="text.secondary">
          {helper}
        </Typography>
      ) : null}
    </Stack>
    <Switch checked={checked} disabled={disabled} onChange={onChange} />
  </Stack>
)

export function SyncViewer({ ref }: { ref?: Ref<DialogRef> }) {
  const { t } = useTranslation()
  const { verge, patchVerge } = useVerge()
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState<SyncBusy>(null)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [pendingFirstSync, setPendingFirstSync] =
    useState<FirstSyncChoice | null>(null)
  const [intervalDraft, setIntervalDraft] = useState<string | null>(null)
  const [deviceDraft, setDeviceDraft] = useState<string | null>(null)
  const [passwordDraft, setPasswordDraft] = useState<string | null>(null)

  const { data: status, refetch } = useQuery({
    queryKey: ['getSyncStatus'],
    queryFn: getSyncStatus,
    enabled: open,
    refetchInterval: open ? 10_000 : false,
    retry: false,
    staleTime: 2000,
  })

  useImperativeHandle(
    ref,
    () => ({
      open: () => {
        setOpen(true)
        setIntervalDraft(null)
        setDeviceDraft(null)
        setPasswordDraft(null)
      },
      close: () => setOpen(false),
    }),
    [],
  )

  const settings = status?.settings ?? verge?.sync_settings
  const enabled = status?.enabled ?? settings?.enabled ?? false
  const mode = status?.mode ?? settings?.mode ?? 'two_way'
  const scopes = status?.scopes ?? settings?.scopes ?? DEFAULT_SYNC_SCOPES
  const intervalMinutes =
    status?.intervalMinutes ?? settings?.interval_minutes ?? 360
  const onStartup = settings?.on_startup ?? true
  const onChange = settings?.on_change ?? false
  const conflictPolicy = settings?.conflict_policy ?? 'last_modified'
  const deviceName = status?.deviceName ?? settings?.device_name ?? ''
  const encrypt = settings?.encrypt ?? false
  const allowInvalidCerts = settings?.danger_accept_invalid_certs ?? false
  const storedPassword = verge?.sync_settings?.password ?? ''
  const passwordValue = passwordDraft ?? storedPassword

  const missingScopes = useMemo(
    () => missingScopesFromError(status?.lastError),
    [status?.lastError],
  )
  const missingScopeLabels = missingScopes
    .map((scope) => t(SCOPE_LABEL_KEYS[scope]))
    .join(', ')

  const actionsDisabled = busy !== null || status?.running === true
  const fieldsDisabled = busy !== null

  const refreshStatus = async () => {
    await refetch().catch(() => undefined)
  }

  const applySyncSettings = async (patch: SyncSettings) => {
    try {
      await patchVerge({ sync_settings: patch })
      await refreshStatus()
    } catch (error) {
      showNotice.error(error)
    }
  }

  const toggleScope = (scope: SyncScope, checked: boolean) => {
    if (!checked && scopes.length <= 1) {
      showNotice.warning('settings.modals.sync.messages.scopeRequired')
      return
    }
    if (!checked && scope === 'overrides' && scopes.includes('custom_rules')) {
      showNotice.warning('settings.modals.sync.messages.scopeDependency')
      return
    }
    let next = checked
      ? orderedScopes([...scopes, scope])
      : orderedScopes(scopes.filter((name) => name !== scope))
    if (checked && scope === 'custom_rules' && !scopes.includes('overrides')) {
      next = orderedScopes([...next, 'overrides'])
    }
    void applySyncSettings({ scopes: next })
  }

  const commitInterval = () => {
    if (intervalDraft === null) return
    const parsed = Number(intervalDraft.trim())
    if (!Number.isFinite(parsed)) {
      setIntervalDraft(null)
      return
    }
    const next = Math.max(MIN_INTERVAL_MINUTES, Math.round(parsed))
    if (next !== intervalMinutes) {
      void applySyncSettings({ interval_minutes: next })
    }
  }

  const commitDeviceName = () => {
    if (deviceDraft === null) return
    const next = deviceDraft.trim().slice(0, MAX_DEVICE_NAME_LENGTH)
    if (next === deviceName) return
    void applySyncSettings({ device_name: next })
  }

  const commitPassword = () => {
    if (passwordDraft === null) return
    if (passwordDraft === storedPassword) return
    void applySyncSettings({ password: passwordDraft })
  }

  const testConnection = useLockFn(async () => {
    setBusy('test')
    try {
      const outcome = await mutate(() => testWebdavSync(), {
        id: 'test-webdav-sync',
        errorNotice: false,
      })
      if (!outcome.ok) return
      showNotice.success('settings.modals.sync.messages.testSuccess')
    } catch (error) {
      showNotice.error('settings.modals.sync.messages.testFailed', {
        error: errorDetail(error),
      })
    } finally {
      setBusy(null)
      await refreshStatus()
    }
  })

  const reportSyncError = (error: unknown) => {
    const detail = errorDetail(error)
    const missing = missingScopesFromError(detail)
    if (missing.length > 0) {
      const labels = missing
        .map((scope) => t(SCOPE_LABEL_KEYS[scope]))
        .join(', ')
      showNotice.warning('settings.modals.sync.messages.scopeMismatch', {
        scopes: labels,
      })
      return
    }
    showNotice.error('settings.modals.sync.messages.syncFailed', {
      error: detail,
    })
  }

  const refreshAppliedData = async () => {
    await Promise.allSettled([
      refetch(),
      revalidateQuery(['getProfiles']),
      revalidateQuery(['getVergeConfig']),
    ])
  }

  const flushDrafts = async () => {
    const patch: SyncSettings = {}
    if (intervalDraft !== null) {
      const parsed = Number(intervalDraft.trim())
      if (Number.isFinite(parsed)) {
        patch.interval_minutes = Math.max(
          MIN_INTERVAL_MINUTES,
          Math.round(parsed),
        )
      }
    }
    if (deviceDraft !== null) {
      patch.device_name = deviceDraft.trim().slice(0, MAX_DEVICE_NAME_LENGTH)
    }
    if (passwordDraft !== null) {
      patch.password = passwordDraft
    }
    if (Object.keys(patch).length > 0) {
      await patchVerge({ sync_settings: patch })
    }
    setIntervalDraft(null)
    setDeviceDraft(null)
    setPasswordDraft(null)
  }

  const runSync = useLockFn(async (choice?: FirstSyncChoice) => {
    if (encrypt && !passwordValue) {
      showNotice.error(
        'settings.modals.sync.messages.encryptionPasswordMissing',
      )
      return
    }
    setBusy('sync')
    try {
      await flushDrafts()
      if (choice) {
        await patchVerge({
          sync_settings: { mode: choice === 'upload' ? 'upload' : 'download' },
        })
      }
      const outcome = await mutate(() => syncNow(), {
        id: 'sync-now',
        errorNotice: false,
      })
      if (!outcome.ok) return
      const result = outcome.value
      switch (result.action) {
        case 'upload':
          showNotice.success('settings.modals.sync.messages.syncUploaded', {
            revision: result.revision,
          })
          break
        case 'download':
          showNotice.success('settings.modals.sync.messages.syncDownloaded', {
            revision: result.revision,
          })
          break
        case 'merge':
          showNotice.success('settings.modals.sync.messages.syncMerged', {
            revision: result.revision,
          })
          break
        case 'restore':
          showNotice.success('settings.modals.sync.messages.syncRestored')
          break
        default:
          showNotice.info('settings.modals.sync.messages.syncUpToDate')
      }
      if (result.conflicts) {
        showNotice.warning('settings.modals.sync.messages.conflictsDetected')
      }
      if (result.applied) {
        await refreshAppliedData()
      } else {
        await refreshStatus()
      }
    } catch (error) {
      reportSyncError(error)
    } finally {
      setBusy(null)
      setPendingFirstSync(null)
    }
  })

  const enableMissingScopes = () => {
    const next = orderedScopes([...scopes, ...missingScopes])
    void applySyncSettings({ scopes: next })
  }

  const remoteSummary = useMemo(() => {
    if (!status) {
      return t('settings.modals.sync.status.remoteUnknown')
    }
    if (status.remoteAvailable == null) {
      return t('settings.modals.sync.status.remoteUnknown')
    }
    if (!status.remoteAvailable) {
      return t('settings.modals.sync.status.remoteUnavailable')
    }
    if (status.remoteRevision != null) {
      return t('settings.modals.sync.status.remoteAvailable', {
        revision: status.remoteRevision,
      })
    }
    return t('settings.modals.sync.status.remoteEmpty')
  }, [status, t])

  const lastSyncedAt =
    formatSyncTime(status?.lastSyncedAt) ??
    t('settings.modals.sync.status.never')
  const firstSyncVisible = Boolean(
    status?.needsFirstSync || status?.remoteRevision === 0,
  )

  return (
    <BaseDialog
      open={open}
      title={t('settings.modals.sync.title')}
      contentSx={{ width: { xs: 360, sm: 640 } }}
      disableOk
      cancelBtn={t('shared.actions.close')}
      onCancel={() => setOpen(false)}
      onClose={() => setOpen(false)}
    >
      <Stack spacing={2}>
        <Alert severity="info">{t('settings.modals.sync.description')}</Alert>

        <Section title={t('settings.modals.sync.sections.status')}>
          <SwitchRow
            label={t('settings.modals.sync.fields.enabled')}
            helper={t('settings.modals.sync.helpers.enabled')}
            checked={enabled}
            disabled={fieldsDisabled}
            onChange={(_, checked) =>
              void applySyncSettings({ enabled: checked })
            }
          />
          <StatusRow
            label={t('settings.modals.sync.fields.state')}
            value={
              status?.running
                ? t('settings.modals.sync.status.running')
                : enabled
                  ? t('settings.modals.sync.status.enabled')
                  : t('settings.modals.sync.status.disabled')
            }
          />
          <StatusRow
            label={t('settings.modals.sync.fields.lastRevision')}
            value={status?.lastRevision ?? 0}
          />
          <StatusRow
            label={t('settings.modals.sync.fields.lastSyncedAt')}
            value={lastSyncedAt}
          />
          <StatusRow
            label={t('settings.modals.sync.fields.remote')}
            value={remoteSummary}
          />
          {status?.remoteError ? (
            <Typography
              variant="caption"
              color="error"
              sx={{ wordBreak: 'break-word' }}
            >
              {status.remoteError}
            </Typography>
          ) : null}
          {status?.lastError ? (
            <Stack spacing={0.5}>
              <Typography variant="body2" color="text.secondary">
                {t('settings.modals.sync.fields.lastError')}
              </Typography>
              <Typography
                variant="caption"
                color="error"
                sx={{ wordBreak: 'break-word' }}
              >
                {status.lastError}
              </Typography>
            </Stack>
          ) : null}
          {missingScopes.length > 0 ? (
            <Alert
              severity="warning"
              action={
                <Button
                  color="inherit"
                  size="small"
                  disabled={fieldsDisabled}
                  onClick={enableMissingScopes}
                >
                  {t('settings.modals.sync.actions.enableMissingScopes')}
                </Button>
              }
            >
              {t('settings.modals.sync.messages.scopeMismatch', {
                scopes: missingScopeLabels,
              })}
            </Alert>
          ) : null}
          <Stack
            direction="row"
            spacing={1}
            useFlexGap
            sx={{ flexWrap: 'wrap' }}
          >
            <Button
              variant="contained"
              size="small"
              disabled={
                status?.running === true || (busy !== null && busy !== 'sync')
              }
              loading={busy === 'sync'}
              onClick={() => void runSync()}
            >
              {t('settings.modals.sync.actions.syncNow')}
            </Button>
            <Button
              variant="outlined"
              size="small"
              disabled={
                status?.running === true || (busy !== null && busy !== 'test')
              }
              loading={busy === 'test'}
              onClick={() => void testConnection()}
            >
              {t('settings.modals.sync.actions.testConnection')}
            </Button>
            <Button
              variant="text"
              size="small"
              disabled={actionsDisabled}
              onClick={() => setHistoryOpen(true)}
            >
              {t('settings.modals.sync.actions.viewHistory')}
            </Button>
          </Stack>
        </Section>

        {firstSyncVisible ? (
          <Section title={t('settings.modals.sync.status.firstSyncTitle')}>
            <Typography variant="body2" color="text.secondary">
              {t('settings.modals.sync.status.firstSyncDescription')}
            </Typography>
            <Stack
              direction="row"
              spacing={1}
              useFlexGap
              sx={{ flexWrap: 'wrap' }}
            >
              <Button
                variant="contained"
                size="small"
                disabled={actionsDisabled}
                onClick={() => setPendingFirstSync('upload')}
              >
                {t('settings.modals.sync.actions.uploadLocal')}
              </Button>
              <Button
                variant="outlined"
                size="small"
                disabled={actionsDisabled}
                onClick={() => setPendingFirstSync('download')}
              >
                {t('settings.modals.sync.actions.downloadCloud')}
              </Button>
            </Stack>
          </Section>
        ) : null}

        <Section title={t('settings.modals.sync.sections.connection')}>
          <Typography variant="caption" color="text.secondary">
            {t('settings.modals.sync.helpers.reuseBackupAccount')}
          </Typography>
          <Typography variant="body2" sx={{ wordBreak: 'break-all' }}>
            {verge?.webdav_url ||
              t('settings.modals.sync.helpers.configureInBackup')}
          </Typography>
          <Stack
            direction="row"
            spacing={1}
            useFlexGap
            sx={{ flexWrap: 'wrap' }}
          >
            <Button
              variant="outlined"
              size="small"
              disabled={status?.running === true || busy !== null}
              loading={busy === 'test'}
              onClick={() => void testConnection()}
            >
              {t('settings.modals.sync.actions.testConnection')}
            </Button>
          </Stack>
          <Typography variant="caption" color="text.secondary">
            {t('settings.modals.sync.helpers.configureInBackup')}
          </Typography>
        </Section>

        <Section title={t('settings.modals.sync.sections.scopes')}>
          {SYNC_SCOPES.map((scope) => (
            <FormControlLabel
              key={scope}
              sx={{ alignItems: 'flex-start', m: 0 }}
              control={
                <Checkbox
                  size="small"
                  checked={scopes.includes(scope)}
                  disabled={fieldsDisabled}
                  onChange={(event) => toggleScope(scope, event.target.checked)}
                />
              }
              label={
                <Stack>
                  <Typography variant="body2">
                    {t(SCOPE_LABEL_KEYS[scope])}
                  </Typography>
                  <Typography variant="caption" color="text.secondary">
                    {t(SCOPE_HELPER_KEYS[scope])}
                  </Typography>
                </Stack>
              }
            />
          ))}
          <Alert severity="info">
            {t('settings.modals.sync.helpers.scopesNever')}
          </Alert>
        </Section>

        <Section title={t('settings.modals.sync.sections.behavior')}>
          <TextField
            select
            size="small"
            label={t('settings.modals.sync.fields.mode')}
            value={mode}
            disabled={fieldsDisabled}
            onChange={(event) =>
              void applySyncSettings({ mode: event.target.value })
            }
            sx={{ minWidth: 200 }}
          >
            <MenuItem value="two_way">
              {t('settings.modals.sync.options.mode.twoWay')}
            </MenuItem>
            <MenuItem value="upload">
              {t('settings.modals.sync.options.mode.upload')}
            </MenuItem>
            <MenuItem value="download">
              {t('settings.modals.sync.options.mode.download')}
            </MenuItem>
          </TextField>
          <TextField
            size="small"
            type="number"
            label={t('settings.modals.sync.fields.interval')}
            value={intervalDraft ?? String(intervalMinutes)}
            disabled={fieldsDisabled}
            helperText={t('settings.modals.sync.helpers.interval')}
            onChange={(event) => setIntervalDraft(event.target.value)}
            onBlur={commitInterval}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault()
                commitInterval()
              }
            }}
            sx={{ maxWidth: 220 }}
            slotProps={{
              htmlInput: {
                min: MIN_INTERVAL_MINUTES,
                inputMode: 'numeric',
              },
            }}
          />
          <SwitchRow
            label={t('settings.modals.sync.fields.onStartup')}
            helper={t('settings.modals.sync.helpers.onStartup')}
            checked={onStartup}
            disabled={fieldsDisabled}
            onChange={(_, checked) =>
              void applySyncSettings({ on_startup: checked })
            }
          />
          <SwitchRow
            label={t('settings.modals.sync.fields.onChange')}
            helper={t('settings.modals.sync.helpers.onChange')}
            checked={onChange}
            disabled={fieldsDisabled}
            onChange={(_, checked) =>
              void applySyncSettings({ on_change: checked })
            }
          />
          <TextField
            select
            size="small"
            label={t('settings.modals.sync.fields.conflictPolicy')}
            value={conflictPolicy}
            disabled={fieldsDisabled}
            onChange={(event) =>
              void applySyncSettings({ conflict_policy: event.target.value })
            }
            sx={{ minWidth: 220 }}
          >
            <MenuItem value="last_modified">
              {t('settings.modals.sync.options.conflictPolicy.lastModified')}
            </MenuItem>
            <MenuItem value="local">
              {t('settings.modals.sync.options.conflictPolicy.local')}
            </MenuItem>
            <MenuItem value="remote">
              {t('settings.modals.sync.options.conflictPolicy.remote')}
            </MenuItem>
          </TextField>
          <TextField
            size="small"
            label={t('settings.modals.sync.fields.deviceName')}
            value={deviceDraft ?? deviceName}
            disabled={fieldsDisabled}
            helperText={t('settings.modals.sync.helpers.deviceName')}
            onChange={(event) => setDeviceDraft(event.target.value)}
            onBlur={commitDeviceName}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault()
                commitDeviceName()
              }
            }}
            sx={{ maxWidth: 320 }}
            slotProps={{
              htmlInput: { maxLength: MAX_DEVICE_NAME_LENGTH },
            }}
          />
        </Section>

        <Section title={t('settings.modals.sync.sections.security')}>
          <SwitchRow
            label={t('settings.modals.sync.fields.encrypt')}
            helper={t('settings.modals.sync.helpers.encryption')}
            checked={encrypt}
            disabled={fieldsDisabled}
            onChange={(_, checked) =>
              void applySyncSettings({ encrypt: checked })
            }
          />
          <TextField
            size="small"
            type="password"
            label={t('settings.modals.sync.fields.encryptionPassword')}
            value={passwordValue}
            disabled={fieldsDisabled || !encrypt}
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck="false"
            onChange={(event) => setPasswordDraft(event.target.value)}
            onBlur={commitPassword}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault()
                commitPassword()
              }
            }}
            sx={{ maxWidth: 320 }}
          />
          {encrypt && !passwordValue ? (
            <Alert severity="warning">
              {t('settings.modals.sync.messages.encryptionPasswordMissing')}
            </Alert>
          ) : null}
          {!encrypt ? (
            <Alert severity="warning">
              {t('settings.modals.sync.messages.plaintextWarning')}
            </Alert>
          ) : null}
          <SwitchRow
            label={t('settings.modals.sync.fields.dangerAcceptInvalidCerts')}
            helper={t('settings.modals.sync.helpers.dangerAcceptInvalidCerts')}
            checked={allowInvalidCerts}
            disabled={fieldsDisabled}
            onChange={(_, checked) =>
              void applySyncSettings({
                danger_accept_invalid_certs: checked,
              })
            }
          />
        </Section>

        <Section title={t('settings.modals.sync.sections.conflicts')}>
          {status?.conflicts.length ? (
            status.conflicts.map((copy) => (
              <Stack key={copy.name} spacing={0.25}>
                <Typography variant="body2" sx={{ wordBreak: 'break-all' }}>
                  {copy.name}
                </Typography>
                <Typography variant="caption" color="text.secondary">
                  {formatSyncTimestamp(copy.createdAt) ??
                    t('settings.modals.sync.status.never')}
                </Typography>
                <Typography
                  variant="caption"
                  color="text.secondary"
                  sx={{ wordBreak: 'break-all' }}
                >
                  {copy.files.length > 20
                    ? `${copy.files.slice(0, 20).join(', ')} …`
                    : copy.files.join(', ')}
                </Typography>
              </Stack>
            ))
          ) : (
            <Typography variant="caption" color="text.secondary">
              {t('settings.modals.sync.status.noConflicts')}
            </Typography>
          )}
        </Section>
      </Stack>

      <SyncHistoryViewer
        open={historyOpen}
        disabled={actionsDisabled}
        onClose={() => setHistoryOpen(false)}
        onRestored={async () => {
          await refreshAppliedData()
          showNotice.info('settings.modals.sync.messages.restoreNoRestart')
        }}
      />

      <BaseDialog
        open={pendingFirstSync !== null}
        title={t('settings.modals.sync.confirm.firstSyncTitle')}
        okBtn={t('shared.actions.confirm')}
        cancelBtn={t('shared.actions.cancel')}
        contentSx={{ width: { xs: 320, sm: 440 } }}
        loading={busy === 'sync'}
        onCancel={() => {
          if (busy !== 'sync') setPendingFirstSync(null)
        }}
        onClose={() => {
          if (busy !== 'sync') setPendingFirstSync(null)
        }}
        onOk={() => {
          if (pendingFirstSync) void runSync(pendingFirstSync)
        }}
      >
        <Typography variant="body2" sx={{ wordBreak: 'break-word' }}>
          {pendingFirstSync === 'download'
            ? t('settings.modals.sync.confirm.firstSyncDownload')
            : t('settings.modals.sync.confirm.firstSyncUpload')}
        </Typography>
      </BaseDialog>
    </BaseDialog>
  )
}
