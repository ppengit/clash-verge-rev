import type { TranslationKey } from '@/types/generated/i18n-keys'
import getSystem from '@/utils/get-system'
import { isValidIpCidr } from '@/utils/network'

export interface RuleTypeMeta {
  name: string
  required?: boolean
  example?: string
  noResolve?: boolean
  validator?: (value: string) => boolean
}

const portValidator = (value: string): boolean => {
  return new RegExp(
    '^(?:[1-9]\\d{0,3}|[1-5]\\d{4}|6[0-4]\\d{3}|65[0-4]\\d{2}|655[0-2]\\d|6553[0-5])$',
  ).test(value)
}

export const RULE_TYPES: RuleTypeMeta[] = [
  {
    name: 'DOMAIN',
    example: 'example.com',
  },
  {
    name: 'DOMAIN-SUFFIX',
    example: 'example.com',
  },
  {
    name: 'DOMAIN-KEYWORD',
    example: 'example',
  },
  {
    name: 'DOMAIN-REGEX',
    example: 'example.*',
  },
  {
    name: 'GEOSITE',
    example: 'youtube',
  },
  {
    name: 'GEOIP',
    example: 'CN',
    noResolve: true,
  },
  {
    name: 'SRC-GEOIP',
    example: 'CN',
  },
  {
    name: 'IP-ASN',
    example: '13335',
    noResolve: true,
    validator: (value) => (+value ? true : false),
  },
  {
    name: 'SRC-IP-ASN',
    example: '9808',
    validator: (value) => (+value ? true : false),
  },
  {
    name: 'IP-CIDR',
    example: '127.0.0.0/8',
    noResolve: true,
    validator: isValidIpCidr,
  },
  {
    name: 'IP-CIDR6',
    example: '2620:0:2d0:200::7/32',
    noResolve: true,
    validator: isValidIpCidr,
  },
  {
    name: 'SRC-IP-CIDR',
    example: '192.168.1.201/32',
    validator: isValidIpCidr,
  },
  {
    name: 'IP-SUFFIX',
    example: '8.8.8.8/24',
    noResolve: true,
    validator: isValidIpCidr,
  },
  {
    name: 'SRC-IP-SUFFIX',
    example: '192.168.1.201/8',
    validator: isValidIpCidr,
  },
  {
    name: 'SRC-PORT',
    example: '7777',
    validator: (value) => portValidator(value),
  },
  {
    name: 'DST-PORT',
    example: '80',
    validator: (value) => portValidator(value),
  },
  {
    name: 'IN-PORT',
    example: '7897',
    validator: (value) => portValidator(value),
  },
  {
    name: 'DSCP',
    example: '4',
  },
  {
    name: 'PROCESS-NAME',
    example: getSystem() === 'windows' ? 'chrome.exe' : 'curl',
  },
  {
    name: 'PROCESS-PATH',
    example:
      getSystem() === 'windows'
        ? 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'
        : '/usr/bin/wget',
  },
  {
    name: 'PROCESS-NAME-REGEX',
    example: '.*telegram.*',
  },
  {
    name: 'PROCESS-PATH-REGEX',
    example:
      getSystem() === 'windows' ? '(?i).*Application\\chrome.*' : '.*bin/wget',
  },
  {
    name: 'NETWORK',
    example: 'udp',
    validator: (value) => ['tcp', 'udp'].includes(value),
  },
  {
    name: 'UID',
    example: '1001',
    validator: (value) => (+value ? true : false),
  },
  {
    name: 'IN-TYPE',
    example: 'SOCKS/HTTP',
  },
  {
    name: 'IN-USER',
    example: 'mihomo',
  },
  {
    name: 'IN-NAME',
    example: 'ss',
  },
  {
    name: 'SUB-RULE',
    example: '(NETWORK,tcp)',
  },
  {
    name: 'RULE-SET',
    example: 'providername',
    noResolve: true,
  },
  {
    name: 'AND',
    example: '((DOMAIN,baidu.com),(NETWORK,UDP))',
  },
  {
    name: 'OR',
    example: '((NETWORK,UDP),(DOMAIN,baidu.com))',
  },
  {
    name: 'NOT',
    example: '((DOMAIN,baidu.com))',
  },
  {
    name: 'MATCH',
    required: false,
  },
]

export const QUICK_ADD_RULE_TYPES = RULE_TYPES.filter(
  (rule) => rule.name !== 'MATCH',
)

export const RULE_TYPE_LABEL_KEYS: Record<string, string> = Object.fromEntries(
  RULE_TYPES.map((rule) => [
    rule.name,
    `rules.modals.editor.ruleTypes.${rule.name}`,
  ]),
)

export const BUILTIN_POLICIES = ['DIRECT', 'REJECT', 'REJECT-DROP', 'PASS']

export const PROXY_POLICY_LABEL_KEYS: Record<string, TranslationKey> =
  BUILTIN_POLICIES.reduce(
    (acc, policy) => {
      acc[policy] =
        `proxies.components.enums.policies.${policy}` as TranslationKey
      return acc
    },
    {} as Record<string, TranslationKey>,
  )

export const buildRuleString = (
  ruleType: string,
  value: string,
  policy: string,
  noResolve: boolean,
) => {
  const trimmed = value.trim()
  return `${ruleType}${trimmed ? `,${trimmed}` : ''},${policy}${
    noResolve ? ',no-resolve' : ''
  }`
}
