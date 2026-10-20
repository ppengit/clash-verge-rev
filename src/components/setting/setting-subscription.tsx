import { useRef } from 'react'
import { useTranslation } from 'react-i18next'

import { type DialogRef } from '@/components/base'
import { useVerge } from '@/hooks/use-verge'

import { SettingItem, SettingList } from './mods/setting-comp'
import { SubscriptionSettingsViewer } from './mods/subscription-settings-viewer'

export const SettingSubscription = () => {
  const { t } = useTranslation()
  const { verge } = useVerge()
  const viewerRef = useRef<DialogRef>(null)

  const subscription = verge?.subscription_settings
  const health = verge?.node_health_settings
  const info = verge?.subscription_info_settings
  const legacyHealthEnabled = verge?.enable_auto_delay_detection ?? false
  const effectiveHealthEnabled = health?.enabled ?? legacyHealthEnabled
  const effectiveHealthInterval =
    health?.interval_minutes ??
    verge?.auto_delay_detection_interval_minutes ??
    10

  const openViewer = () => viewerRef.current?.open()

  return (
    <SettingList title={t('settings.sections.subscription.title')}>
      <SubscriptionSettingsViewer ref={viewerRef} />

      <SettingItem
        onClick={openViewer}
        label={t('settings.sections.subscription.fields.updates')}
        secondary={t('settings.sections.subscription.summaries.updates', {
          interval: Math.max(
            1440,
            subscription?.default_update_interval ?? 1440,
          ),
        })}
      />
      <SettingItem
        onClick={openViewer}
        label={t('settings.sections.subscription.fields.health')}
        secondary={
          effectiveHealthEnabled
            ? t('settings.sections.subscription.summaries.healthEnabled', {
                interval: effectiveHealthInterval,
              })
            : t('settings.sections.subscription.summaries.healthDisabled')
        }
      />
      <SettingItem
        onClick={openViewer}
        label={t('settings.sections.subscription.fields.display')}
        secondary={t('settings.sections.subscription.summaries.display')}
      />
      <SettingItem
        onClick={openViewer}
        label={t('settings.sections.subscription.fields.info')}
        secondary={
          info?.show_traffic
            ? t('settings.sections.subscription.summaries.infoEnabled', {
                days: info.expiry_warning_days ?? 3,
              })
            : t('settings.sections.subscription.summaries.infoDisabled')
        }
      />
    </SettingList>
  )
}

export default SettingSubscription
