import customVersion from '@root/custom-version.json'
import { version as upstreamVersion } from '@root/package.json'

export const APP_VERSION = `${upstreamVersion}-custom.${customVersion.custom}`
