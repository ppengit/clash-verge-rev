import { AddRounded } from '@mui/icons-material'
import {
  Autocomplete,
  Box,
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  TextField,
} from '@mui/material'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import type { Rule as MihomoRule } from 'tauri-plugin-mihomo-api'

import {
  BaseEmpty,
  BasePage,
  BaseSearchBox,
  VirtualList,
  type VirtualListHandle,
} from '@/components/base'
import { ScrollTopButton } from '@/components/layout/scroll-top-button'
import { ProviderButton } from '@/components/rule/provider-button'
import { QuickAddRuleDialog } from '@/components/rule/quick-add-rule-dialog'
import RuleItem, { type RuleSource } from '@/components/rule/rule-item'
import {
  BUILTIN_POLICIES,
  PROXY_POLICY_LABEL_KEYS,
  RULE_TYPE_LABEL_KEYS,
} from '@/components/rule/rule-meta'
import { useProfiles } from '@/hooks/use-profiles'
import { useVisibility } from '@/hooks/use-visibility'
import {
  useAppRefreshers,
  useProxiesData,
  useRulesData,
} from '@/providers/app-data-context'
import type { CustomRule } from '@/services/bindings'
import {
  deleteCustomRule,
  listCustomRules,
  readProfileFile,
  updateCustomRulePolicy,
} from '@/services/cmds'
import { mutate } from '@/services/mutate'
import { showNotice } from '@/services/notice-service'
import { useQuery } from '@/services/query-client'
import { parseYamlSafe } from '@/utils/yaml'

type RuleFilter = 'all' | 'custom' | 'subscription'

interface LegacyRuleFile {
  prepend?: string[]
  append?: string[]
}

type RuleIdentitySource = Pick<MihomoRule, 'type' | 'payload' | 'proxy'>

const RUNTIME_RULE_TYPES: Record<string, string> = {
  Domain: 'DOMAIN',
  DomainSuffix: 'DOMAIN-SUFFIX',
  DomainKeyword: 'DOMAIN-KEYWORD',
  DomainRegex: 'DOMAIN-REGEX',
  DomainWildcard: 'DOMAIN-WILDCARD',
  GeoSite: 'GEOSITE',
  GeoIP: 'GEOIP',
  SrcGeoIP: 'SRC-GEOIP',
  IPASN: 'IP-ASN',
  SrcIPASN: 'SRC-IP-ASN',
  IPCIDR: 'IP-CIDR',
  SrcIPCIDR: 'SRC-IP-CIDR',
  IPSuffix: 'IP-SUFFIX',
  SrcIPSuffix: 'SRC-IP-SUFFIX',
  SrcPort: 'SRC-PORT',
  DstPort: 'DST-PORT',
  InPort: 'IN-PORT',
  DSCP: 'DSCP',
  InUser: 'IN-USER',
  InName: 'IN-NAME',
  InType: 'IN-TYPE',
  ProcessName: 'PROCESS-NAME',
  ProcessPath: 'PROCESS-PATH',
  ProcessNameRegex: 'PROCESS-NAME-REGEX',
  ProcessPathRegex: 'PROCESS-PATH-REGEX',
  ProcessNameWildcard: 'PROCESS-NAME-WILDCARD',
  ProcessPathWildcard: 'PROCESS-PATH-WILDCARD',
  Match: 'MATCH',
  RuleSet: 'RULE-SET',
  Network: 'NETWORK',
  Uid: 'UID',
  SubRules: 'SUB-RULE',
  AND: 'AND',
  OR: 'OR',
  NOT: 'NOT',
}

const ruleTypeName = (type: MihomoRule['type']) => {
  if (typeof type !== 'string') return 'UNKNOWN'
  return RUNTIME_RULE_TYPES[type] ?? type
}

const runtimeIdentity = (rule: RuleIdentitySource) =>
  `${ruleTypeName(rule.type)}|${rule.payload ?? ''}|${rule.proxy ?? ''}`

const customIdentity = (rule: Pick<CustomRule, 'type' | 'value' | 'policy'>) =>
  `${rule.type}|${rule.value}|${rule.policy}`

const legacyIdentity = (raw: string) => {
  const parts = raw.split(',').map((part) => part.trim())
  let end = parts.length
  if (parts[end - 1]?.toLowerCase() === 'no-resolve') end -= 1
  if (end < 2) return raw
  return `${parts[0]}|${parts.slice(1, end - 1).join(',')}|${parts[end - 1]}`
}

const runtimeRuleString = (rule: RuleIdentitySource) =>
  `${ruleTypeName(rule.type)}${rule.payload ? `,${rule.payload}` : ''},${rule.proxy}`

const RulesPage = () => {
  const { t } = useTranslation()
  const { rules = [] } = useRulesData()
  const { proxyView } = useProxiesData()
  const { current } = useProfiles()
  const { refreshRules, refreshRuleProviders } = useAppRefreshers()
  const [match, setMatch] = useState(() => (_: string) => true)
  const [filter, setFilter] = useState<RuleFilter>('all')
  const virtuosoRef = useRef<VirtualListHandle>(null)
  const [showScrollTop, setShowScrollTop] = useState(false)
  const pageVisible = useVisibility()
  const [quickAddOpen, setQuickAddOpen] = useState(false)
  const [rebinding, setRebinding] = useState<CustomRule | null>(null)
  const [rebindPolicy, setRebindPolicy] = useState('DIRECT')

  const currentUid = current?.uid

  const { data: customRules, refetch: refetchCustomRules } = useQuery<
    CustomRule[]
  >({
    queryKey: ['customRules', currentUid],
    queryFn: () => listCustomRules(currentUid ?? null),
    enabled: Boolean(currentUid),
  })

  const legacyRulesUid = current?.option?.rules || 'Rules'

  const { data: legacyRules } = useQuery<LegacyRuleFile | null>({
    queryKey: ['legacyRules', legacyRulesUid],
    queryFn: async () => {
      if (!legacyRulesUid) return null
      const data = await readProfileFile(legacyRulesUid)
      return parseYamlSafe(data) as LegacyRuleFile | null
    },
    enabled: Boolean(legacyRulesUid),
  })

  const refetchCustomRulesRef = useRef(refetchCustomRules)
  refetchCustomRulesRef.current = refetchCustomRules

  const refreshAllRules = useCallback(async () => {
    await Promise.all([
      refreshRules(),
      refreshRuleProviders(),
      refetchCustomRulesRef.current(),
    ])
  }, [refreshRules, refreshRuleProviders])

  useEffect(() => {
    if (!pageVisible) return
    void refreshAllRules()
  }, [refreshAllRules, pageVisible])

  const customByRule = useMemo(() => {
    const map = new Map<string, CustomRule>()
    ;(customRules ?? []).forEach((rule) => {
      if (rule.deleted || !rule.enabled) return
      map.set(customIdentity(rule), rule)
    })
    return map
  }, [customRules])

  const legacyRuleStrings = useMemo(() => {
    const set = new Set<string>()
    ;[...(legacyRules?.prepend ?? []), ...(legacyRules?.append ?? [])].forEach(
      (rule) => {
        if (typeof rule === 'string') set.add(legacyIdentity(rule))
      },
    )
    return set
  }, [legacyRules])

  const sourceOf = useCallback(
    (rule: RuleIdentitySource): RuleSource => {
      const identity = runtimeIdentity(rule)
      const custom = customByRule.get(identity)
      if (custom) return custom.profile === '*' ? 'global' : 'custom'
      if (legacyRuleStrings.has(identity)) return 'legacy'
      return 'subscription'
    },
    [customByRule, legacyRuleStrings],
  )

  const filteredRules = useMemo(() => {
    const rulesWithLineNo = rules.map((item, index) => ({
      ...item,
      // UI-only derived data; keep app context/SWR data immutable
      lineNo: index + 1,
    }))

    return rulesWithLineNo.filter((item) => {
      const payload = item.payload ?? ''
      if (!match(payload)) return false
      const source = sourceOf(item)
      if (filter === 'custom') return source !== 'subscription'
      if (filter === 'subscription') return source === 'subscription'
      return true
    })
  }, [rules, match, sourceOf, filter])

  const policyOptions = useMemo(
    () =>
      [
        ...BUILTIN_POLICIES,
        ...(proxyView?.groups.map((group) => group.name) ?? []),
      ].filter((item, index, all) => all.indexOf(item) === index),
    [proxyView?.groups],
  )

  const handleScroll = useCallback((e: Event) => {
    setShowScrollTop((e.target as HTMLElement).scrollTop > 100)
  }, [])

  const scrollToTop = () => {
    virtuosoRef.current?.scrollTo({ top: 0, behavior: 'smooth' })
  }

  const copyRule = async (rule: RuleIdentitySource) => {
    await writeText(runtimeRuleString(rule))
    showNotice.success('rules.page.feedback.notifications.ruleCopied')
  }

  const removeRule = async (rule: CustomRule) => {
    try {
      await mutate(() => deleteCustomRule(rule.id), {
        id: `delete-custom-rule:${rule.id}`,
        errorNotice: false,
      })
      await refreshAllRules()
      showNotice.success('rules.page.feedback.notifications.ruleDeleted')
    } catch (error) {
      showNotice.error(error)
    }
  }

  const startRebind = (rule: CustomRule) => {
    setRebinding(rule)
    setRebindPolicy(rule.policy)
  }

  const confirmRebind = async () => {
    if (!rebinding) return
    try {
      await mutate(() => updateCustomRulePolicy(rebinding.id, rebindPolicy), {
        id: `rebind-custom-rule:${rebinding.id}`,
        errorNotice: false,
      })
      setRebinding(null)
      await refreshAllRules()
    } catch (error) {
      showNotice.error(error)
    }
  }

  return (
    <BasePage
      full
      title={t('rules.page.title')}
      contentStyle={{
        height: '100%',
        display: 'flex',
        flexDirection: 'column',
        overflow: 'auto',
      }}
      header={
        <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
          <ProviderButton />
          <Button
            size="small"
            variant="contained"
            startIcon={<AddRounded />}
            onClick={() => setQuickAddOpen(true)}
          >
            {t('rules.page.actions.addRule')}
          </Button>
        </Box>
      }
    >
      <Box
        sx={{
          pt: 1,
          mb: 0.5,
          mx: '10px',
          minHeight: '36px',
          display: 'flex',
          alignItems: 'center',
          gap: 1,
        }}
      >
        <BaseSearchBox onSearch={(match) => setMatch(() => match)} />
        <Box sx={{ display: 'flex', gap: 0.5, flexShrink: 0 }}>
          {(['all', 'custom', 'subscription'] as const).map((value) => (
            <Chip
              key={value}
              size="small"
              label={t(`rules.page.filters.${value}`)}
              color={filter === value ? 'primary' : 'default'}
              variant={filter === value ? 'filled' : 'outlined'}
              onClick={() => setFilter(value)}
            />
          ))}
        </Box>
      </Box>

      {filteredRules && filteredRules.length > 0 ? (
        <>
          <VirtualList
            ref={virtuosoRef}
            count={filteredRules.length}
            estimateSize={40}
            renderItem={(i) => {
              const rule = filteredRules[i]
              const custom = customByRule.get(runtimeIdentity(rule))
              return (
                <RuleItem
                  value={rule}
                  source={sourceOf(rule)}
                  onCopy={() => copyRule(rule)}
                  onDelete={custom ? () => removeRule(custom) : undefined}
                  onRebind={custom ? () => startRebind(custom) : undefined}
                />
              )
            }}
            style={{ flex: 1 }}
            onScroll={handleScroll}
          />
          <ScrollTopButton onClick={scrollToTop} show={showScrollTop} />
        </>
      ) : (
        <BaseEmpty
          extra={
            <Button
              size="small"
              variant="outlined"
              startIcon={<AddRounded />}
              sx={{ mt: 1 }}
              onClick={() => setQuickAddOpen(true)}
            >
              {t('rules.page.actions.addRule')}
            </Button>
          }
        />
      )}

      <QuickAddRuleDialog
        open={quickAddOpen}
        defaultProfileUid={currentUid}
        onClose={() => setQuickAddOpen(false)}
        onChanged={() => void refreshAllRules()}
      />

      <Dialog
        open={Boolean(rebinding)}
        onClose={() => setRebinding(null)}
        maxWidth="xs"
        fullWidth
      >
        <DialogTitle>{t('rules.page.actions.rebindPolicy')}</DialogTitle>
        <DialogContent>
          <Autocomplete
            size="small"
            options={policyOptions}
            value={rebindPolicy}
            getOptionLabel={(option) =>
              t(PROXY_POLICY_LABEL_KEYS[option] ?? option)
            }
            onChange={(_, option) => {
              if (typeof option === 'string') setRebindPolicy(option)
            }}
            renderInput={(params) => (
              <TextField
                {...params}
                label={t('rules.page.actions.rebindPolicy')}
                sx={{ mt: 1 }}
              />
            )}
          />
          {rebinding && (
            <Box sx={{ mt: 1, fontSize: 12, opacity: 0.7 }}>
              {RULE_TYPE_LABEL_KEYS[rebinding.type]
                ? t(RULE_TYPE_LABEL_KEYS[rebinding.type])
                : rebinding.type}
              ,{rebinding.value},{rebinding.policy}
            </Box>
          )}
        </DialogContent>
        <DialogActions>
          <Button variant="outlined" onClick={() => setRebinding(null)}>
            {t('shared.actions.cancel')}
          </Button>
          <Button variant="contained" onClick={confirmRebind}>
            {t('shared.actions.save')}
          </Button>
        </DialogActions>
      </Dialog>
    </BasePage>
  )
}

export default RulesPage
