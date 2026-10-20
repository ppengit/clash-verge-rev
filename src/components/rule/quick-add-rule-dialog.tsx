import {
  Alert,
  Autocomplete,
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControl,
  FormControlLabel,
  InputLabel,
  MenuItem,
  Select,
  Snackbar,
  Stack,
  TextField,
  Typography,
} from '@mui/material'
import { useCallback, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { Switch } from '@/components/base'
import {
  BUILTIN_POLICIES,
  buildRuleString,
  PROXY_POLICY_LABEL_KEYS,
  QUICK_ADD_RULE_TYPES,
  RULE_TYPE_LABEL_KEYS,
} from '@/components/rule/rule-meta'
import { useProfiles } from '@/hooks/use-profiles'
import { useProxiesData, useRulesData } from '@/providers/app-data-context'
import type { CustomRule, RulePosition, RuleTargets } from '@/services/bindings'
import {
  addCustomRule,
  deleteCustomRule,
  getRuleTargets,
  listCustomRules,
} from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { showNotice } from '@/services/notice-service'
import { useQuery } from '@/services/query-client'

export interface QuickAddRulePrefill {
  ruleType: string
  value: string
  noResolve?: boolean
}

interface Props {
  open: boolean
  defaultProfileUid?: string | null
  prefill?: QuickAddRulePrefill
  onClose: () => void
  onChanged?: () => void
}

interface FormProps extends Props {
  onAdded: (rule: CustomRule) => void
}

const errorMessage = (error: unknown) => {
  if (typeof error === 'string') return error
  if (error && typeof error === 'object' && 'detail' in error) {
    return String((error as { detail: unknown }).detail)
  }
  return String(error)
}

const QuickAddRuleForm = ({
  open,
  defaultProfileUid,
  prefill,
  onClose,
  onChanged,
  onAdded,
}: FormProps) => {
  const { t } = useTranslation()
  const { proxyView } = useProxiesData()
  const { rules } = useRulesData()
  const { profiles, current } = useProfiles()

  const [profileUid, setProfileUid] = useState(
    defaultProfileUid ?? current?.uid ?? '',
  )
  const [ruleType, setRuleType] = useState(
    prefill?.ruleType ?? QUICK_ADD_RULE_TYPES[0]?.name ?? 'DOMAIN-SUFFIX',
  )
  const [value, setValue] = useState(prefill?.value ?? '')
  const [policy, setPolicy] = useState('DIRECT')
  const [position, setPosition] = useState<RulePosition>('prepend')
  const [noResolve, setNoResolve] = useState(prefill?.noResolve ?? false)
  const [applyToAll, setApplyToAll] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [advanced, setAdvanced] = useState(false)

  const profileOptions = useMemo(
    () =>
      (profiles?.items ?? []).filter(
        (item): item is NonNullable<typeof item> =>
          Boolean(item?.uid) &&
          (item?.type === 'remote' || item?.type === 'local'),
      ),
    [profiles?.items],
  )

  const isCurrent = Boolean(profileUid && profileUid === current?.uid)

  const { data: targets } = useQuery<RuleTargets | null>({
    queryKey: ['ruleTargets', profileUid],
    queryFn: () => getRuleTargets(profileUid),
    enabled: Boolean(profileUid),
  })

  const { data: customRules } = useQuery<CustomRule[]>({
    queryKey: ['customRules', profileUid],
    queryFn: () => listCustomRules(profileUid),
    enabled: Boolean(profileUid),
  })

  const ruleMeta = useMemo(
    () => QUICK_ADD_RULE_TYPES.find((rule) => rule.name === ruleType),
    [ruleType],
  )

  const policies = useMemo(() => {
    const runtimeGroups = isCurrent
      ? (proxyView?.groups.map((group) => group.name) ?? [])
      : (targets?.groups ?? [])
    return [...BUILTIN_POLICIES, ...runtimeGroups].filter(
      (item, index, all) => all.indexOf(item) === index,
    )
  }, [isCurrent, proxyView?.groups, targets?.groups])

  const policyKind = BUILTIN_POLICIES.includes(policy) ? 'builtin' : 'group'
  const allowFreeSolo = !isCurrent && Boolean(targets?.hasScript)

  const preview = useMemo(
    () => buildRuleString(ruleType, value, policy, noResolve),
    [ruleType, value, policy, noResolve],
  )

  const duplicate = useMemo(
    () =>
      (customRules ?? []).find(
        (rule) =>
          !rule.deleted &&
          rule.enabled &&
          rule.type === ruleType &&
          rule.value.trim() === value.trim() &&
          rule.policy === policy &&
          rule.no_resolve === noResolve,
      ),
    [customRules, ruleType, value, policy, noResolve],
  )

  const subscriptionMatch = useMemo(() => {
    if (!isCurrent) return false
    const suffix = value.trim().toLowerCase()
    if (!suffix) return false
    return rules.some((rule) => {
      const payload =
        typeof rule.payload === 'string' ? rule.payload.toLowerCase() : ''
      return payload.includes(suffix)
    })
  }, [isCurrent, rules, value])

  const validate = useCallback(() => {
    if (!profileUid) {
      return t('rules.modals.quickAdd.validation.profileRequired')
    }
    if (!value.trim()) {
      return t('rules.modals.quickAdd.validation.valueRequired')
    }
    if (ruleMeta?.validator && !ruleMeta.validator(value.trim())) {
      return t('rules.modals.quickAdd.validation.invalidValue')
    }
    if (!policy) {
      return t('rules.modals.quickAdd.validation.policyRequired')
    }
    if (
      policyKind === 'group' &&
      !policies.includes(policy) &&
      !allowFreeSolo
    ) {
      return t('rules.modals.quickAdd.validation.unknownPolicy')
    }
    if (duplicate) {
      return t('rules.modals.quickAdd.validation.duplicate')
    }
    if (applyToAll && policyKind !== 'builtin') {
      return t('rules.modals.quickAdd.validation.globalBuiltinOnly')
    }
    return null
  }, [
    allowFreeSolo,
    applyToAll,
    duplicate,
    policy,
    policyKind,
    policies,
    profileUid,
    ruleMeta,
    t,
    value,
  ])

  const onSubmit = async () => {
    const validationError = validate()
    if (validationError) {
      setError(validationError)
      return
    }

    setSubmitting(true)
    setError(null)
    try {
      const result = await mutate(
        () =>
          addCustomRule({
            profileUid,
            ruleType,
            value: value.trim(),
            policy,
            policyKind,
            position,
            noResolve: noResolve && Boolean(ruleMeta?.noResolve),
            applyToAll,
          }),
        { id: 'add-custom-rule', errorNotice: false },
      )
      if (!result.ok) return
      onAdded(result.value)
      onChanged?.()
      onClose()
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Dialog open={open} onClose={onClose} maxWidth="sm" fullWidth>
      <DialogTitle>{t('rules.modals.quickAdd.title')}</DialogTitle>
      <DialogContent
        sx={{
          display: 'grid',
          gap: 1.5,
          pt: 2,
          alignContent: 'start',
          scrollbarGutter: 'stable',
        }}
      >
        {error && <Alert severity="error">{error}</Alert>}

        <FormControl size="small" fullWidth>
          <InputLabel>{t('rules.modals.quickAdd.fields.profile')}</InputLabel>
          <Select
            label={t('rules.modals.quickAdd.fields.profile')}
            value={profileUid}
            onChange={(event) => {
              setProfileUid(event.target.value)
              setPolicy('DIRECT')
              setApplyToAll(false)
            }}
          >
            {profileOptions.map((item) => (
              <MenuItem key={item.uid ?? ''} value={item.uid ?? ''}>
                {item.name ?? item.uid}
                {item.uid === current?.uid
                  ? ` (${t('rules.modals.quickAdd.currentProfile')})`
                  : ''}
              </MenuItem>
            ))}
          </Select>
        </FormControl>

        <Autocomplete
          size="small"
          options={QUICK_ADD_RULE_TYPES}
          value={ruleMeta ?? null}
          getOptionLabel={(option) =>
            t(RULE_TYPE_LABEL_KEYS[option.name] ?? option.name)
          }
          onChange={(_, option) => {
            if (!option) return
            setRuleType(option.name)
            setNoResolve(false)
          }}
          renderInput={(params) => (
            <TextField
              {...params}
              label={t('rules.modals.quickAdd.fields.type')}
            />
          )}
        />

        <TextField
          size="small"
          label={t('rules.modals.quickAdd.fields.value')}
          value={value}
          placeholder={ruleMeta?.example}
          helperText={
            ruleMeta?.example
              ? t('rules.modals.quickAdd.valueHint', {
                  example: ruleMeta.example,
                })
              : undefined
          }
          onChange={(event) => setValue(event.target.value)}
        />

        <Autocomplete
          size="small"
          freeSolo={allowFreeSolo}
          options={policies}
          value={policy}
          getOptionLabel={(option) =>
            t(PROXY_POLICY_LABEL_KEYS[option] ?? option)
          }
          onChange={(_, option) => {
            if (typeof option !== 'string') return
            setPolicy(option)
            if (!BUILTIN_POLICIES.includes(option)) setApplyToAll(false)
          }}
          onInputChange={(_, input) => {
            if (!allowFreeSolo) return
            setPolicy(input)
            if (!BUILTIN_POLICIES.includes(input)) setApplyToAll(false)
          }}
          renderInput={(params) => (
            <TextField
              {...params}
              label={t('rules.modals.quickAdd.fields.policy')}
            />
          )}
        />

        <FormControl size="small" fullWidth>
          <InputLabel>{t('rules.modals.quickAdd.fields.position')}</InputLabel>
          <Select
            label={t('rules.modals.quickAdd.fields.position')}
            value={position}
            onChange={(event) =>
              setPosition(event.target.value as RulePosition)
            }
          >
            <MenuItem value="prepend">
              {t('rules.modals.quickAdd.position.prepend')}
            </MenuItem>
            <MenuItem value="append">
              {t('rules.modals.quickAdd.position.append')}
            </MenuItem>
          </Select>
        </FormControl>

        {ruleMeta?.noResolve && (
          <FormControlLabel
            control={
              <Switch
                checked={noResolve}
                onChange={() => setNoResolve((current) => !current)}
              />
            }
            label={t('rules.modals.editor.form.toggles.noResolve')}
          />
        )}

        <Stack
          direction="row"
          spacing={1}
          useFlexGap
          sx={{ alignItems: 'center', flexWrap: 'wrap' }}
        >
          <Button
            size="small"
            onClick={() => setAdvanced((current) => !current)}
          >
            {t('rules.modals.quickAdd.advanced')}
          </Button>
          {advanced && (
            <FormControlLabel
              control={
                <Switch
                  checked={applyToAll}
                  disabled={policyKind !== 'builtin'}
                  onChange={() => setApplyToAll((current) => !current)}
                />
              }
              label={t('rules.modals.quickAdd.applyToAll')}
              sx={{ m: 0 }}
            />
          )}
        </Stack>

        <Box
          sx={{
            p: 1.5,
            borderRadius: 1,
            border: 1,
            borderColor: 'divider',
            wordBreak: 'break-all',
          }}
        >
          <Typography variant="caption" color="text.secondary">
            {t('rules.modals.quickAdd.preview')}
          </Typography>
          <Typography variant="body2" sx={{ fontFamily: 'monospace' }}>
            {preview}
          </Typography>
        </Box>

        {duplicate && (
          <Alert severity="warning">
            {t('rules.modals.quickAdd.duplicateHint', {
              id: duplicate.id,
            })}
          </Alert>
        )}
        {subscriptionMatch && (
          <Alert severity="info">
            {t('rules.modals.quickAdd.subscriptionHint')}
          </Alert>
        )}
      </DialogContent>
      <DialogActions>
        <Button variant="outlined" onClick={onClose}>
          {t('shared.actions.cancel')}
        </Button>
        <Button variant="contained" onClick={onSubmit} disabled={submitting}>
          {t('rules.modals.quickAdd.actions.add')}
        </Button>
      </DialogActions>
    </Dialog>
  )
}

export const QuickAddRuleDialog = ({
  open,
  defaultProfileUid,
  prefill,
  onClose,
  onChanged,
}: Props) => {
  const { t } = useTranslation()
  const [lastAdded, setLastAdded] = useState<CustomRule | null>(null)
  const sessionKey = `${open}:${defaultProfileUid ?? ''}:${
    prefill?.ruleType ?? ''
  }:${prefill?.value ?? ''}`

  const onUndo = async () => {
    if (!lastAdded) return
    try {
      await mutate(() => deleteCustomRule(lastAdded.id), {
        id: `undo-custom-rule:${lastAdded.id}`,
        errorNotice: false,
      })
      setLastAdded(null)
      onChanged?.()
      showNotice.success('rules.modals.quickAdd.feedback.undone')
    } catch (err) {
      showNotice.error(err)
    }
  }

  return (
    <>
      {open && (
        <QuickAddRuleForm
          key={sessionKey}
          open={open}
          defaultProfileUid={defaultProfileUid}
          prefill={prefill}
          onClose={onClose}
          onChanged={onChanged}
          onAdded={setLastAdded}
        />
      )}
      <Snackbar
        open={Boolean(lastAdded)}
        autoHideDuration={8000}
        onClose={() => setLastAdded(null)}
        message={t('rules.modals.quickAdd.feedback.added')}
        action={
          <Button color="inherit" size="small" onClick={onUndo}>
            {t('rules.modals.quickAdd.actions.undo')}
          </Button>
        }
      />
    </>
  )
}
