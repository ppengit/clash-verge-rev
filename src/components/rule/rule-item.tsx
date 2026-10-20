import { MoreVert } from '@mui/icons-material'
import {
  Chip,
  IconButton,
  Menu,
  MenuItem,
  styled,
  Box,
  Typography,
} from '@mui/material'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Rule } from 'tauri-plugin-mihomo-api'

const Item = styled(Box)(({ theme }) => ({
  display: 'flex',
  padding: '4px 16px',
  color: theme.palette.text.primary,
}))

const COLOR = [
  'primary',
  'secondary',
  'info.main',
  'warning.main',
  'success.main',
]

export type RuleSource = 'subscription' | 'legacy' | 'custom' | 'global'

interface Props {
  value: Rule & { lineNo: number }
  source?: RuleSource
  onCopy?: () => void
  onDelete?: () => void
  onRebind?: () => void
}

const parseColor = (text: string) => {
  if (text === 'REJECT' || text === 'REJECT-DROP') return 'error.main'
  if (text === 'DIRECT') return 'text.primary'

  let sum = 0
  for (let i = 0; i < text.length; i++) {
    sum += text.charCodeAt(i)
  }
  return COLOR[sum % COLOR.length]
}

const SOURCE_LABEL_KEYS: Record<RuleSource, string> = {
  subscription: 'rules.page.badges.subscription',
  legacy: 'rules.page.badges.legacy',
  custom: 'rules.page.badges.custom',
  global: 'rules.page.badges.global',
}

const RuleItem = (props: Props) => {
  const { value, source, onCopy, onDelete, onRebind } = props
  const { t } = useTranslation()
  const [anchorEl, setAnchorEl] = useState<null | HTMLElement>(null)
  const menuOpen = Boolean(anchorEl)

  return (
    <Item sx={{ borderBottom: '1px solid var(--divider-color)' }}>
      <Typography
        color="text.secondary"
        variant="body2"
        sx={{ lineHeight: 2, minWidth: 30, mr: 2.25, textAlign: 'center' }}
      >
        {value.lineNo}
      </Typography>

      <Box sx={{ userSelect: 'text', flex: 1 }}>
        <Typography component="h6" variant="subtitle1" color="text.primary">
          {value.payload || '-'}
        </Typography>

        <Typography
          component="span"
          variant="body2"
          color="text.secondary"
          sx={{ mr: 2, minWidth: 120, display: 'inline-block' }}
        >
          {typeof value.type === 'string' ? value.type : value.type.Unknown}
        </Typography>

        <Typography
          component="span"
          variant="body2"
          color={parseColor(value.proxy)}
          sx={{ mr: 2 }}
        >
          {value.proxy}
        </Typography>

        {source && (
          <Chip
            size="small"
            variant="outlined"
            label={t(SOURCE_LABEL_KEYS[source])}
            sx={{ height: 18, fontSize: 10 }}
          />
        )}
      </Box>

      {(onCopy || onDelete || onRebind) && (
        <>
          <IconButton
            size="small"
            onClick={(event) => setAnchorEl(event.currentTarget)}
            aria-label={t('rules.page.actions.menu')}
          >
            <MoreVert fontSize="small" />
          </IconButton>
          <Menu
            anchorEl={anchorEl}
            open={menuOpen}
            onClose={() => setAnchorEl(null)}
          >
            {onCopy && (
              <MenuItem
                onClick={() => {
                  setAnchorEl(null)
                  onCopy()
                }}
              >
                {t('rules.page.actions.copyRule')}
              </MenuItem>
            )}
            {onRebind && (
              <MenuItem
                onClick={() => {
                  setAnchorEl(null)
                  onRebind()
                }}
              >
                {t('rules.page.actions.rebindPolicy')}
              </MenuItem>
            )}
            {onDelete && (
              <MenuItem
                onClick={() => {
                  setAnchorEl(null)
                  onDelete()
                }}
              >
                {t('rules.page.actions.deleteRule')}
              </MenuItem>
            )}
          </Menu>
        </>
      )}
    </Item>
  )
}

export default RuleItem
