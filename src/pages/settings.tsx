import { GitHub, HelpOutlineRounded, Telegram } from '@mui/icons-material'
import { Box, ButtonGroup, IconButton } from '@mui/material'
import { useLockFn } from 'ahooks'
import { useTranslation } from 'react-i18next'

import { BasePage } from '@/components/base'
import SettingClash from '@/components/setting/setting-clash'
import SettingSubscription from '@/components/setting/setting-subscription'
import SettingSystem from '@/components/setting/setting-system'
import SettingVergeAdvanced from '@/components/setting/setting-verge-advanced'
import SettingVergeBasic from '@/components/setting/setting-verge-basic'
import { showNotice } from '@/services/notice-service'
import { useThemeMode } from '@/services/states'
import { openExternalUrl } from '@/utils/open-external-url'

const SettingPage = () => {
  const { t } = useTranslation()

  const onError = (err: any) => {
    showNotice.error(err)
  }

  const toGithubRepo = useLockFn(() =>
    openExternalUrl('https://github.com/clash-verge-rev/clash-verge-rev').catch(
      onError,
    ),
  )

  const toGithubDoc = useLockFn(() =>
    openExternalUrl('https://clash-verge-rev.github.io/index.html').catch(
      onError,
    ),
  )

  const toTelegramChannel = useLockFn(() =>
    openExternalUrl('https://t.me/clash_verge_re').catch(onError),
  )

  const mode = useThemeMode()
  const isDark = mode === 'light' ? false : true

  return (
    <BasePage
      title={t('settings.page.title')}
      header={
        <ButtonGroup
          variant="contained"
          aria-label={t('settings.page.actionsGroupLabel')}
        >
          <IconButton
            size="medium"
            color="inherit"
            title={t('settings.page.actions.manual')}
            onClick={toGithubDoc}
          >
            <HelpOutlineRounded fontSize="inherit" />
          </IconButton>
          <IconButton
            size="medium"
            color="inherit"
            title={t('settings.page.actions.telegram')}
            onClick={toTelegramChannel}
          >
            <Telegram fontSize="inherit" />
          </IconButton>

          <IconButton
            size="medium"
            color="inherit"
            title={t('settings.page.actions.github')}
            onClick={toGithubRepo}
          >
            <GitHub fontSize="inherit" />
          </IconButton>
        </ButtonGroup>
      }
    >
      <Box
        sx={{
          columnCount: { xs: 1, md: 2 },
          columnFill: 'balance',
          columnGap: 1.5,
        }}
      >
        {[
          { key: 'system', section: <SettingSystem key="system" onError={onError} /> },
          { key: 'subscription', section: <SettingSubscription key="subscription" /> },
          { key: 'clash', section: <SettingClash key="clash" onError={onError} /> },
          { key: 'basic', section: <SettingVergeBasic key="basic" onError={onError} /> },
          {
            key: 'advanced',
            section: <SettingVergeAdvanced key="advanced" onError={onError} />,
          },
        ].map(({ key, section }) => (
          <Box
            key={key}
            sx={{
              breakInside: 'avoid',
              borderRadius: 2,
              mb: 1.5,
              overflow: 'hidden',
              backgroundColor: isDark ? '#282a36' : '#ffffff',
            }}
          >
            {section}
          </Box>
        ))}
      </Box>
    </BasePage>
  )
}

export default SettingPage
