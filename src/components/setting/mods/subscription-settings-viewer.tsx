import { AddRounded, DeleteOutlineRounded } from '@mui/icons-material'
import {
  Box,
  Button,
  IconButton,
  InputAdornment,
  List,
  ListItem,
  ListItemText,
  MenuItem,
  Select,
  TextField,
  Typography,
} from '@mui/material'
import { useLockFn } from 'ahooks'
import { forwardRef, useImperativeHandle, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { BaseDialog, Switch, type DialogRef } from '@/components/base'
import { useVerge } from '@/hooks/use-verge'
import type {
  NodeDisplaySettings,
  NodeHealthSettings,
  ProbeTarget,
  SubscriptionInfoSettings,
  SubscriptionSettings,
} from '@/services/bindings'
import { runNodeHealthCheck } from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { showNotice } from '@/services/notice-service'
import { isValidLatencyTestUrl } from '@/utils/network'

const MAX_TARGETS = 5

const isValidExpectedStatus = (raw: string) => {
  const value = raw.trim()
  if (!value) return false
  return value.split('/').every((part) => {
    const [start, end] = part.split('-')
    const lower = Number(start)
    const upper = end === undefined ? lower : Number(end)
    return (
      Number.isInteger(lower) &&
      Number.isInteger(upper) &&
      lower >= 100 &&
      lower <= 599 &&
      upper >= 100 &&
      upper <= 599 &&
      lower <= upper
    )
  })
}

type TargetDraft = {
  id: string
  url: string
  expected: string
  keyword: string
  timeout: string
}

type Values = {
  defaultUpdateInterval: string
  updateOnStartup: boolean
  updateOnlyCurrent: boolean
  notifyOnFailure: boolean
  healthEnabled: boolean
  l1Interval: string
  l2Interval: string
  scope: string
  verifyOnSelect: boolean
  latencyUrl: string
  latencyTimeout: string
  l1Targets: TargetDraft[]
  l2Targets: TargetDraft[]
  hideTimeout: boolean
  hideUntested: boolean
  hideUnhealthy: boolean
  hideSlowMs: string
  defaultSort: string
  showTraffic: boolean
  expiryWarningDays: string
}

type Errors = Record<string, string>

let nextTargetId = 0

const emptyTarget = (): TargetDraft => ({
  id: `target-${nextTargetId++}`,
  url: '',
  expected: '200',
  keyword: '',
  timeout: '',
})

const defaultL1Drafts = (): TargetDraft[] => [
  {
    ...emptyTarget(),
    url: 'https://cp.cloudflare.com/generate_204',
    expected: '204',
  },
]

const defaultL2Drafts = (): TargetDraft[] => [
  {
    ...emptyTarget(),
    url: 'https://www.google.com/robots.txt',
    expected: '200',
  },
  {
    ...emptyTarget(),
    url: 'https://github.com/favicon.ico',
    expected: '200',
  },
  {
    ...emptyTarget(),
    url: 'https://www.cloudflare.com/cdn-cgi/trace',
    expected: '200',
  },
]

const toDrafts = (
  targets: ProbeTarget[] | null | undefined,
  fallback: () => TargetDraft[],
): TargetDraft[] => {
  const drafts = (targets ?? []).slice(0, MAX_TARGETS).map((target) => ({
    id: `target-${nextTargetId++}`,
    url: target.url,
    expected: target.expected,
    keyword: target.keyword ?? '',
    timeout: target.timeout_ms ? String(target.timeout_ms) : '',
  }))
  return drafts.length > 0 ? drafts : fallback()
}

const isBlankTarget = (target: TargetDraft) =>
  !target.url.trim() &&
  !target.expected.trim() &&
  !target.keyword.trim() &&
  !target.timeout.trim()

const parseIntInRange = (raw: string, min: number, max: number) => {
  const value = Number(raw)
  if (!Number.isInteger(value) || value < min || value > max) return undefined
  return value
}

export const SubscriptionSettingsViewer = forwardRef<DialogRef>((_, ref) => {
  const { t } = useTranslation()
  const { verge, patchVerge } = useVerge()

  const [open, setOpen] = useState(false)
  const [errors, setErrors] = useState<Errors>({})
  const [values, setValues] = useState<Values>(() => ({
    defaultUpdateInterval: '1440',
    updateOnStartup: false,
    updateOnlyCurrent: false,
    notifyOnFailure: true,
    healthEnabled: false,
    l1Interval: '10',
    l2Interval: '60',
    scope: 'current_group',
    verifyOnSelect: true,
    latencyUrl: '',
    latencyTimeout: '10000',
    l1Targets: defaultL1Drafts(),
    l2Targets: defaultL2Drafts(),
    hideTimeout: false,
    hideUntested: false,
    hideUnhealthy: false,
    hideSlowMs: '0',
    defaultSort: '0',
    showTraffic: true,
    expiryWarningDays: '3',
  }))

  useImperativeHandle(ref, () => ({
    open: () => {
      const subscription = verge?.subscription_settings
      const health = verge?.node_health_settings
      const display = verge?.node_display_settings
      const info = verge?.subscription_info_settings
      const legacyHealthEnabled = verge?.enable_auto_delay_detection ?? false
      setErrors({})
      setValues({
        defaultUpdateInterval: String(
          Math.max(1440, subscription?.default_update_interval ?? 1440),
        ),
        updateOnStartup: subscription?.update_on_startup ?? false,
        updateOnlyCurrent: subscription?.update_only_current ?? false,
        notifyOnFailure: subscription?.notify_on_failure ?? true,
        healthEnabled: health?.enabled ?? legacyHealthEnabled,
        l1Interval: String(
          health?.interval_minutes ??
            verge?.auto_delay_detection_interval_minutes ??
            10,
        ),
        l2Interval: String(health?.deep_interval_minutes ?? 60),
        scope:
          health?.scope ??
          (legacyHealthEnabled ? 'current_node' : 'current_group'),
        verifyOnSelect: health?.verify_on_select ?? true,
        latencyUrl: verge?.default_latency_test ?? '',
        latencyTimeout: String(verge?.default_latency_timeout ?? 10000),
        l1Targets: toDrafts(health?.l1_targets, defaultL1Drafts),
        l2Targets: toDrafts(health?.l2_targets, defaultL2Drafts),
        hideTimeout: display?.hide_timeout ?? false,
        hideUntested: display?.hide_untested ?? false,
        hideUnhealthy: display?.hide_unhealthy ?? false,
        hideSlowMs: String(display?.hide_slow_ms ?? 0),
        defaultSort: String(display?.default_sort ?? 0),
        showTraffic: info?.show_traffic ?? true,
        expiryWarningDays: String(info?.expiry_warning_days ?? 3),
      })
      setOpen(true)
    },
    close: () => setOpen(false),
  }))

  const update = <Key extends keyof Values>(key: Key, value: Values[Key]) =>
    setValues((previous) => ({ ...previous, [key]: value }))

  const updateTarget = (
    level: 'l1Targets' | 'l2Targets',
    index: number,
    patch: Partial<TargetDraft>,
  ) =>
    setValues((previous) => ({
      ...previous,
      [level]: previous[level].map((target, targetIndex) =>
        targetIndex === index ? { ...target, ...patch } : target,
      ),
    }))

  const removeTarget = (level: 'l1Targets' | 'l2Targets', index: number) =>
    setValues((previous) => ({
      ...previous,
      [level]: previous[level].filter(
        (_, targetIndex) => targetIndex !== index,
      ),
    }))

  const addTarget = (level: 'l1Targets' | 'l2Targets') =>
    setValues((previous) =>
      previous[level].length >= MAX_TARGETS
        ? previous
        : { ...previous, [level]: [...previous[level], emptyTarget()] },
    )

  const validate = (): Errors => {
    const next: Errors = {}
    if (
      parseIntInRange(values.defaultUpdateInterval, 1440, 43200) === undefined
    ) {
      next.defaultUpdateInterval = t(
        'settings.modals.subscription.validation.updateInterval',
      )
    }
    if (parseIntInRange(values.l1Interval, 1, 1440) === undefined) {
      next.l1Interval = t('settings.modals.subscription.validation.l1Interval')
    }
    if (parseIntInRange(values.l2Interval, 1, 1440) === undefined) {
      next.l2Interval = t('settings.modals.subscription.validation.l2Interval')
    }
    if (parseIntInRange(values.latencyTimeout, 1000, 30000) === undefined) {
      next.latencyTimeout = t(
        'settings.modals.subscription.validation.latencyTimeout',
      )
    }
    if (values.latencyUrl.trim() && !isValidLatencyTestUrl(values.latencyUrl)) {
      next.latencyUrl = t('settings.modals.subscription.validation.url')
    }
    if (parseIntInRange(values.hideSlowMs, 0, 60000) === undefined) {
      next.hideSlowMs = t('settings.modals.subscription.validation.hideSlowMs')
    }
    if (parseIntInRange(values.expiryWarningDays, 1, 365) === undefined) {
      next.expiryWarningDays = t(
        'settings.modals.subscription.validation.expiryWarningDays',
      )
    }

    const validateTargets = (level: 'l1Targets' | 'l2Targets') => {
      const rows = values[level].filter((target) => !isBlankTarget(target))
      if (rows.length === 0) {
        next[level] = t(
          level === 'l1Targets'
            ? 'settings.modals.subscription.validation.requireL1'
            : 'settings.modals.subscription.validation.requireL2',
        )
        return
      }
      values[level].forEach((target, index) => {
        if (isBlankTarget(target)) return
        const prefix = `${level}.${index}`
        if (!isValidLatencyTestUrl(target.url)) {
          next[`${prefix}.url`] = t(
            'settings.modals.subscription.validation.url',
          )
        }
        if (!isValidExpectedStatus(target.expected)) {
          next[`${prefix}.expected`] = t(
            'settings.modals.subscription.validation.expected',
          )
        }
        if (
          target.timeout.trim() &&
          parseIntInRange(target.timeout, 1000, 30000) === undefined
        ) {
          next[`${prefix}.timeout`] = t(
            'settings.modals.subscription.validation.targetTimeout',
          )
        }
      })
    }
    validateTargets('l1Targets')
    validateTargets('l2Targets')

    return next
  }

  const toTargets = (rows: TargetDraft[]): ProbeTarget[] =>
    rows
      .filter((target) => !isBlankTarget(target))
      .slice(0, MAX_TARGETS)
      .map((target) => {
        const timeout = target.timeout.trim()
          ? parseIntInRange(target.timeout, 1000, 30000)
          : undefined
        const keyword = target.keyword.trim()
        return {
          url: target.url.trim(),
          expected: target.expected.trim(),
          keyword: keyword || null,
          timeout_ms: timeout ?? null,
        }
      })

  const onRunNow = useLockFn(async () => {
    try {
      await mutate(() => runNodeHealthCheck(), {
        id: 'run-node-health-check',
        errorNotice: false,
      })
      showNotice.success('settings.modals.subscription.feedback.healthStarted')
    } catch (error) {
      showNotice.error(error)
    }
  })

  const onSave = useLockFn(async () => {
    const nextErrors = validate()
    setErrors(nextErrors)
    if (Object.keys(nextErrors).length > 0) return

    const subscription_settings: SubscriptionSettings = {
      default_update_interval:
        parseIntInRange(values.defaultUpdateInterval, 1440, 43200) ?? 1440,
      update_on_startup: values.updateOnStartup,
      update_only_current: values.updateOnlyCurrent,
      notify_on_failure: values.notifyOnFailure,
    }
    const node_health_settings: NodeHealthSettings = {
      enabled: values.healthEnabled,
      interval_minutes: parseIntInRange(values.l1Interval, 1, 1440) ?? 10,
      deep_interval_minutes: parseIntInRange(values.l2Interval, 1, 1440) ?? 60,
      scope: values.scope,
      verify_on_select: values.verifyOnSelect,
      l1_targets: toTargets(values.l1Targets),
      l2_targets: toTargets(values.l2Targets),
    }
    const node_display_settings: NodeDisplaySettings = {
      hide_timeout: values.hideTimeout,
      hide_untested: values.hideUntested,
      hide_unhealthy: values.hideUnhealthy,
      hide_slow_ms: parseIntInRange(values.hideSlowMs, 0, 60000) ?? 0,
      default_sort: Number(values.defaultSort),
    }
    const subscription_info_settings: SubscriptionInfoSettings = {
      show_traffic: values.showTraffic,
      expiry_warning_days:
        parseIntInRange(values.expiryWarningDays, 1, 365) ?? 3,
    }

    try {
      await patchVerge({
        subscription_settings,
        node_health_settings,
        node_display_settings,
        subscription_info_settings,
        default_latency_test: values.latencyUrl.trim(),
        default_latency_timeout:
          parseIntInRange(values.latencyTimeout, 1000, 30000) ?? 10000,
      })
      setOpen(false)
    } catch (err) {
      showNotice.error(err)
    }
  })

  const targetEditor = (level: 'l1Targets' | 'l2Targets', title: string) => (
    <Box sx={{ mt: 1 }}>
      <Box
        sx={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <Typography variant="body2" color="text.secondary">
          {title}
        </Typography>
        <Button
          size="small"
          startIcon={<AddRounded />}
          disabled={values[level].length >= MAX_TARGETS}
          onClick={() => addTarget(level)}
        >
          {t('settings.modals.subscription.actions.addTarget')}
        </Button>
      </Box>
      {errors[level] && (
        <Typography variant="caption" color="error">
          {errors[level]}
        </Typography>
      )}
      {values[level].map((target, index) => (
        <Box
          key={target.id}
          sx={{ mt: 1, p: 1, borderRadius: 1, bgcolor: 'action.hover' }}
        >
          <Box sx={{ display: 'flex', gap: 1, alignItems: 'flex-start' }}>
            <TextField
              autoComplete="new-password"
              size="small"
              fullWidth
              label={t('settings.modals.subscription.fields.targetUrl')}
              placeholder="https://example.com/path"
              value={target.url}
              error={!!errors[`${level}.${index}.url`]}
              helperText={errors[`${level}.${index}.url`]}
              onChange={(event) =>
                updateTarget(level, index, { url: event.target.value })
              }
            />
            <IconButton
              size="small"
              title={t('shared.actions.delete')}
              onClick={() => removeTarget(level, index)}
            >
              <DeleteOutlineRounded fontSize="small" />
            </IconButton>
          </Box>
          <Box sx={{ display: 'flex', gap: 1, mt: 1 }}>
            <TextField
              autoComplete="new-password"
              size="small"
              label={t('settings.modals.subscription.fields.targetExpected')}
              sx={{ width: 130 }}
              value={target.expected}
              error={!!errors[`${level}.${index}.expected`]}
              helperText={errors[`${level}.${index}.expected`]}
              onChange={(event) =>
                updateTarget(level, index, { expected: event.target.value })
              }
            />
            <TextField
              autoComplete="new-password"
              size="small"
              label={t('settings.modals.subscription.fields.targetKeyword')}
              sx={{ flex: 1 }}
              value={target.keyword}
              onChange={(event) =>
                updateTarget(level, index, { keyword: event.target.value })
              }
            />
            <TextField
              autoComplete="new-password"
              size="small"
              type="number"
              label={t('settings.modals.subscription.fields.targetTimeout')}
              sx={{ width: 130 }}
              value={target.timeout}
              error={!!errors[`${level}.${index}.timeout`]}
              helperText={errors[`${level}.${index}.timeout`]}
              onChange={(event) =>
                updateTarget(level, index, { timeout: event.target.value })
              }
              slotProps={{
                input: {
                  endAdornment: (
                    <InputAdornment position="end">
                      {t('shared.units.milliseconds')}
                    </InputAdornment>
                  ),
                },
                htmlInput: { min: 1000, max: 30000 },
              }}
            />
          </Box>
        </Box>
      ))}
    </Box>
  )

  const sectionTitle = (key: string) => (
    <Typography
      variant="subtitle2"
      sx={{ mt: 2, mb: 0.5, fontWeight: 700 }}
      color="text.primary"
    >
      {t(key)}
    </Typography>
  )

  return (
    <BaseDialog
      open={open}
      title={t('settings.modals.subscription.title')}
      contentSx={{ width: { xs: 360, sm: 620 }, maxHeight: '70vh' }}
      okBtn={t('shared.actions.save')}
      cancelBtn={t('shared.actions.cancel')}
      onClose={() => setOpen(false)}
      onCancel={() => setOpen(false)}
      onOk={onSave}
    >
      <List disablePadding>
        {sectionTitle('settings.modals.subscription.sections.updates')}
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t(
              'settings.modals.subscription.fields.defaultUpdateInterval',
            )}
            sx={{ maxWidth: 'fit-content' }}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            type="number"
            sx={{ width: 150, ml: 'auto' }}
            value={values.defaultUpdateInterval}
            error={!!errors.defaultUpdateInterval}
            helperText={errors.defaultUpdateInterval}
            onChange={(event) =>
              update('defaultUpdateInterval', event.target.value)
            }
            slotProps={{
              input: {
                endAdornment: (
                  <InputAdornment position="end">
                    {t('shared.units.minutes')}
                  </InputAdornment>
                ),
              },
            }}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.updateOnStartup')}
          />
          <Switch
            edge="end"
            checked={values.updateOnStartup}
            onChange={(_, checked) => update('updateOnStartup', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.updateOnlyCurrent')}
          />
          <Switch
            edge="end"
            checked={values.updateOnlyCurrent}
            onChange={(_, checked) => update('updateOnlyCurrent', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.notifyOnFailure')}
          />
          <Switch
            edge="end"
            checked={values.notifyOnFailure}
            onChange={(_, checked) => update('notifyOnFailure', checked)}
          />
        </ListItem>

        {sectionTitle('settings.modals.subscription.sections.health')}
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.healthEnabled')}
          />
          <Switch
            edge="end"
            checked={values.healthEnabled}
            onChange={(_, checked) => update('healthEnabled', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.testInterval')}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            type="number"
            sx={{ width: 150, ml: 'auto' }}
            value={values.l1Interval}
            error={!!errors.l1Interval}
            helperText={errors.l1Interval}
            onChange={(event) => update('l1Interval', event.target.value)}
            slotProps={{
              input: {
                endAdornment: (
                  <InputAdornment position="end">
                    {t('shared.units.minutes')}
                  </InputAdornment>
                ),
              },
            }}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.deepTestInterval')}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            type="number"
            sx={{ width: 150, ml: 'auto' }}
            value={values.l2Interval}
            error={!!errors.l2Interval}
            helperText={errors.l2Interval}
            onChange={(event) => update('l2Interval', event.target.value)}
            slotProps={{
              input: {
                endAdornment: (
                  <InputAdornment position="end">
                    {t('shared.units.minutes')}
                  </InputAdornment>
                ),
              },
            }}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.scope')}
          />
          <Select
            size="small"
            sx={{ width: 180, '> div': { py: '7.5px' } }}
            value={values.scope}
            onChange={(event) => update('scope', event.target.value as string)}
          >
            <MenuItem value="current_node">
              {t('settings.modals.subscription.options.scope.currentNode')}
            </MenuItem>
            <MenuItem value="current_group">
              {t('settings.modals.subscription.options.scope.currentGroup')}
            </MenuItem>
            <MenuItem value="all_groups">
              {t('settings.modals.subscription.options.scope.allGroups')}
            </MenuItem>
          </Select>
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.verifyOnSelect')}
          />
          <Switch
            edge="end"
            checked={values.verifyOnSelect}
            onChange={(_, checked) => update('verifyOnSelect', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.runNow')}
          />
          <Button
            size="small"
            variant="outlined"
            sx={{ ml: 'auto' }}
            onClick={onRunNow}
          >
            {t('settings.modals.subscription.actions.runNow')}
          </Button>
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.latencyTestUrl')}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            sx={{ width: 280, ml: 'auto' }}
            value={values.latencyUrl}
            placeholder="https://cp.cloudflare.com/generate_204"
            error={!!errors.latencyUrl}
            helperText={errors.latencyUrl}
            onChange={(event) => update('latencyUrl', event.target.value)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.latencyTimeout')}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            type="number"
            sx={{ width: 150, ml: 'auto' }}
            value={values.latencyTimeout}
            error={!!errors.latencyTimeout}
            helperText={errors.latencyTimeout}
            onChange={(event) => update('latencyTimeout', event.target.value)}
            slotProps={{
              input: {
                endAdornment: (
                  <InputAdornment position="end">
                    {t('shared.units.milliseconds')}
                  </InputAdornment>
                ),
              },
            }}
          />
        </ListItem>
        {targetEditor(
          'l1Targets',
          t('settings.modals.subscription.fields.l1Targets'),
        )}
        {targetEditor(
          'l2Targets',
          t('settings.modals.subscription.fields.l2Targets'),
        )}

        {sectionTitle('settings.modals.subscription.sections.display')}
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.hideTimeout')}
          />
          <Switch
            edge="end"
            checked={values.hideTimeout}
            onChange={(_, checked) => update('hideTimeout', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.hideUntested')}
          />
          <Switch
            edge="end"
            checked={values.hideUntested}
            onChange={(_, checked) => update('hideUntested', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.hideUnhealthy')}
          />
          <Switch
            edge="end"
            checked={values.hideUnhealthy}
            onChange={(_, checked) => update('hideUnhealthy', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.hideSlow')}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            type="number"
            sx={{ width: 150, ml: 'auto' }}
            value={values.hideSlowMs}
            error={!!errors.hideSlowMs}
            helperText={errors.hideSlowMs}
            onChange={(event) => update('hideSlowMs', event.target.value)}
            slotProps={{
              input: {
                endAdornment: (
                  <InputAdornment position="end">
                    {t('shared.units.milliseconds')}
                  </InputAdornment>
                ),
              },
            }}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.defaultSort')}
          />
          <Select
            size="small"
            sx={{ width: 180, '> div': { py: '7.5px' } }}
            value={values.defaultSort}
            onChange={(event) =>
              update('defaultSort', event.target.value as string)
            }
          >
            <MenuItem value="0">
              {t('settings.modals.subscription.options.sort.default')}
            </MenuItem>
            <MenuItem value="1">
              {t('settings.modals.subscription.options.sort.delay')}
            </MenuItem>
            <MenuItem value="2">
              {t('settings.modals.subscription.options.sort.name')}
            </MenuItem>
          </Select>
        </ListItem>

        {sectionTitle('settings.modals.subscription.sections.info')}
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.showTraffic')}
          />
          <Switch
            edge="end"
            checked={values.showTraffic}
            onChange={(_, checked) => update('showTraffic', checked)}
          />
        </ListItem>
        <ListItem sx={{ padding: '5px 2px' }}>
          <ListItemText
            primary={t('settings.modals.subscription.fields.expiryWarningDays')}
          />
          <TextField
            autoComplete="new-password"
            size="small"
            type="number"
            sx={{ width: 150, ml: 'auto' }}
            value={values.expiryWarningDays}
            error={!!errors.expiryWarningDays}
            helperText={errors.expiryWarningDays}
            onChange={(event) =>
              update('expiryWarningDays', event.target.value)
            }
          />
        </ListItem>
      </List>
    </BaseDialog>
  )
})
