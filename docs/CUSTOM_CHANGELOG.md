# 定制功能与变更记录（Fork Changelog）

> 本文件记录本仓库相对上游 `clash-verge-rev` 的全部定制功能与变更，供下次合并上游版本时定位冲突、移植代码和复核行为。
>
> - 上游基线版本：`v2.5.9`（`package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`，不因定制而修改）
> - 当前定制版本：`custom.1`（`custom-version.json`）
> - 完整展示版本：`v2.5.9-custom.1`
> - 记录日期：`2026-10-20`
> - 适用分支：`dev` 及基于 `dev` 的功能分支
> - 本文件是 fork 专属文档，不属于上游功能；合并上游时不要删除。

---

## 0. 版本命名与发布规则

本仓库的版本号分成两层，互不覆盖：

| 层级 | 含义 | 唯一来源 | 示例 |
|---|---|---|---|
| 上游版本 | 当前合并到的 clash-verge-rev 版本 | `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json` | `2.5.9` |
| 定制版本 | 同一上游版本上第几次发布定制 | `custom-version.json` 的 `custom` | `1` |

界面与「复制版本」显示 `v2.5.9-custom.1`，由 `src/utils/version.ts` 拼接；自动更新仍按上游版本 `2.5.9` 比较，因此上游发布 `2.5.10` 时能正常检测到更新。

规则：

1. 不直接修改上游版本号，否则无法判断上游是否发布了新版本。上游版本只在上游合并时随之更新。
2. 每次发布一组定制变更，`custom` 加 1；同一版本内继续开发不重复递增。
3. 合并到新的上游版本后，`custom` 从 1 重新开始，例如 `2.5.10-custom.1`。
4. 根目录 `Changelog.md` 只保留上游内容，不写入 fork 条目；定制记录写在本文件。
5. 安装包文件名仍使用上游版本；定制版本只在应用界面、诊断信息和本文件中体现。

---

## 1. 变更总览

| 主题 | 用户可见变化 | 主要新增文件 | 主要修改文件 |
|---|---|---|---|
| 独立自定义规则 | 规则页/连接详情快速添加规则；规则独立保存；订阅更新前检查失效分组 | `src-tauri/src/enhance/custom_rules.rs`、`src-tauri/src/cmd/custom_rules.rs`、`src/components/rule/quick-add-rule-dialog.tsx`、`src/components/rule/rule-meta.ts` | `config/prfitem.rs`、`config/profiles.rs`、`enhance/mod.rs`、`feat/profile.rs`、`pages/rules.tsx`、`components/connection/connection-detail.tsx` |
| 订阅与节点设置 | 新增「订阅与节点」设置：订阅更新、定时测速、真实站点健康检查、节点显示过滤 | `src-tauri/src/core/node_health.rs`、`src-tauri/src/core/node_health_probe.rs`、`src-tauri/src/cmd/node_health.rs`、`src/components/setting/setting-subscription.tsx`、`src/components/setting/mods/subscription-settings-viewer.tsx`、`src/services/node-health.ts`、`src/providers/node-health-*.ts(x)` | `config/verge.rs`、`feat/executor.rs`、`utils/resolve/mod.rs`、`feat/window.rs`、`core/timer.rs`、`proxy/*`、`settings.tsx`、`profiles.tsx` |
| WebDAV 备份与同步 | 原「备份设置」改为「备份与同步」；新增定时双向同步订阅元数据、脚本/覆写、自定义规则；冲突保留；可选加密 | `src-tauri/src/core/sync/*`（11 个模块）、`src-tauri/src/cmd/sync.rs`、`src/components/setting/mods/sync-viewer.tsx`、`sync-history-viewer.tsx`、`sync-helpers.ts` | `core/backup.rs`、`cmd/webdav.rs`、`cmd/profile.rs`、`cmd/save_profile.rs`、`config/verge.rs`、`feat/executor.rs`、`utils/dirs.rs`、`utils/resolve/mod.rs`、`feat/window.rs`、`components/setting/mods/backup-viewer.tsx`、`pages/settings.tsx` |
| 配套优化 | 手动测速默认改用 HTTPS；订阅/节点设置纳入配置；备份包含自定义规则 | — | `services/delay.ts`、`proxy-head.tsx`、`proxy-group-tools.tsx`、`tray/latency.rs`、`core/backup.rs`、`timer.rs` |

---

## 2. 独立自定义规则（Standalone Custom Rules）

### 2.1 行为

- 自定义规则不再写入订阅自己的 `r<uid>.yaml`，而是写入应用数据根目录的 `custom-rules.yaml`。
- 每条规则绑定订阅的稳定身份 `PrfItem.sync_id`（`*` 表示所有订阅），不绑定本地 `uid`。
- 规则在配置增强阶段注入：`prepend` 插入最前；`append` 插入第一条 `MATCH` 之前。
- 无效策略/规则集/子规则会被跳过并限频告警，不会让整份配置校验失败。
- 规则页支持来源标记、筛选、复制、删除、重新绑定策略；连接详情支持按域名/IP/进程/端口预填。
- 订阅更新前会解析新订阅的分组/provider/sub-rule，发现失效引用时弹出重映射/改 DIRECT/删除/取消对话框；定时更新则保留旧订阅并通知。
- 旧 `r<uid>.yaml` 规则继续生效；快速添加只写新文件。

### 2.2 数据与兼容

- 新增文件：`<app_home>/custom-rules.yaml`，schema `version: 1`。
- 新增可选字段：`PrfItem.sync_id`；老订阅在启动/首次使用时补生成并写回 `profiles.yaml`。
- 上限：2 MB、10,000 条规则、value 2,048 字符；90 天墓碑压缩。
- 旧规则文件、旧备份、旧 `profiles.yaml` 保持兼容。
- 快速添加命令拒绝 `MATCH`；高级 YAML 编辑仍可使用 `MATCH`。

### 2.3 上游合并注意

- `src-tauri/src/enhance/mod.rs`：自定义规则注入点位于 profile/global Script 之后、最终清理之前。上游若调整增强顺序，需要重新评估注入位置。
- `src-tauri/src/config/prfitem.rs` / `profiles.rs`：`sync_id` 字段、生成、迁移、`update_item`/`patch_item` 保留逻辑。上游若修改 `PrfItem` 字段列表或 `update_item`，必须确保不会覆盖 `sync_id`。
- `src-tauri/src/feat/profile.rs`：订阅更新预检、staged update、冲突解决、文件恢复。上游若重构订阅更新流程，需要把 `preflight_update`、`STAGED_UPDATES`、`resolve_update_conflicts` 迁移过去。
- 前端 `pages/rules.tsx` 使用 runtime 规则类型映射（PascalCase → `DOMAIN-SUFFIX` 等）；上游若修改 `tauri-plugin-mihomo-api` 的 `RuleType`，需要同步 `RUNTIME_RULE_TYPES`。

---

## 3. 订阅与节点设置 / 自动化健康检查

### 3.1 行为

- 设置页新增「订阅与节点」卡片，订阅页齿轮打开同一弹窗，分四个区域：
  1. 订阅更新：默认更新间隔、启动更新、仅当前订阅、失败通知。
  2. 定时测速与健康：启用、L1/L2 间隔、范围、验证目标、选中后验证、默认测速 URL/超时。
  3. 节点显示：隐藏超时/未测速/不健康/高延迟节点、默认排序。
  4. 订阅信息：显示流量/到期、到期提醒天数。
- 新增 Rust `NodeHealthManager`：
  - L1 使用 HTTPS 204（`expected=204`）。
  - L2 使用真实站点（Google/GitHub/Cloudflare，`expected=200`）。
  - L3 对选中节点通过本地 mixed port 做内容探测（状态码、可选关键字、64 KB 上限）。
  - 健康状态：`healthy`、`degraded`、`unusable`、`site_blocked`、`unverified`、`testing`。
  - 单飞、代际取消、并发 10/5、60 秒周期上限、5,000 条缓存、2 MB/5 分钟持久化、失败退避、退出有界等待。
- 代理页显示健康徽标；延迟排序优先健康状态；每组工具栏新增过滤图标；当前节点始终保留；搜索绕过隐藏；显示隐藏数量。
- 选中节点后可异步验证并提示不可用节点。
- 旧 `enable_auto_delay_detection` 迁移为健康检查；首页旧前端定时器已移除。
- 订阅行为：默认更新间隔 1440 分钟、启动更新、仅当前订阅、失败通知均接入 Timer/更新流程。

### 3.2 数据与兼容

- 新增 `IVerge` 嵌套设置：`subscription_settings`、`node_health_settings`、`node_display_settings`、`subscription_info_settings`。
- 新增本地缓存：`<app_home>/node-health.json`。
- 默认延迟测试 URL 从 HTTP 改为 HTTPS（手动测速、分组测速、托盘测速、设置占位符）。
- 老 `enable_auto_delay_detection` / `auto_delay_detection_interval_minutes` 字段保留一个版本用于迁移。

### 3.3 上游合并注意

- `src-tauri/src/config/verge.rs`：新增结构、模板默认值、字段级 patch 合并、`verge_registry!` 分类。上游若增加/重排 `IVerge` 字段，必须同步 `feat/effects.rs` 宏解构，否则编译失败。
- `src-tauri/src/core/timer.rs`：`update_only_current` 过滤、启动更新、失败通知。上游若重构 Timer，需要保留这些分支。
- `src-tauri/src/core/node_health_probe.rs`：直接调用 mihomo 控制器并传 `expected`；默认 TCP，`external-controller` 为空时使用 IPC。上游若修改插件 delay API，优先保留本实现。
- 前端 `use-filter-sort.ts` / `use-render-list.ts` / `use-head-state.ts`：显示过滤、健康排序、每组 override。上游若重写代理页渲染，需要迁移这些参数。
- 前端 `bindings.ts` 中 node-health/sync 类型是手工补齐的（当前环境 `gen:contract` 因 Windows loader 报 `0xC0000139` 无法运行）；合并上游后应在可运行环境重新生成并比对。

---

## 4. WebDAV 备份与同步（Backup & Sync）

### 4.1 行为

- 原「备份设置」入口改为「备份与同步」，原备份功能保留：
  - 自动本地备份、手动本地/WebDAV 备份、历史查看、导入/导出、恢复。
  - 新备份现在包含 `custom-rules.yaml`；恢复旧备份仍可用。
- 新增「云同步」：
  - 复用同一 WebDAV 账号（凭据仍只保存在本机，不上传）。
  - 远端目录固定为 `clash-verge-sync`，与备份目录 `clash-verge-rev-backup` 分离。
  - 默认同步范围：订阅元数据（名称、URL、更新选项、排序）、用户编写的 Script/Merge/Proxies/Groups、自定义规则。
  - 默认不同步：订阅抓取内容 `R<uid>.yaml`、provider 缓存、流量/到期、WebDAV 凭据、设备本地设置。
  - 可选第四范围：应用设置白名单。
  - 模式：双向/仅上传/仅下载；启动同步、间隔（最小 15 分钟）、变更后同步（防抖）。
  - 冲突策略：最后修改优先/本机优先/云端优先；冲突副本保存在本地 `sync/conflicts`，远端保留历史版本（10 个）。
  - 可选密码加密：PBKDF2-HMAC-SHA256（200k）+ AES-256-GCM；密码只存本机，备份 ZIP 会移除 `sync_settings.password`。
- 同步应用：
  - 先下载到 staging，校验大小/SHA-256/解密/ZIP 路径与大小。
  - 应用前创建完整本地备份；应用后重新 enhance，失败恢复备份并重新 enhance。
  - 不自动重启应用。
  - 订阅按 `sync_id` 匹配，缺失时按规范化 URL + 类型；新订阅创建本地 uid 并保留 sync_id，内容由本机重新拉取。
- 安全护栏：
  - 设备 scope 少于云端 scope 时拒绝上传，避免删除云端其他 scope 数据。
  - 新启用 scope 且云端已有该 scope 时，要求先下载/双向合并，避免本机默认值覆盖云端。
  - 自定义规则 scope 依赖 overrides scope。
  - 覆盖文件上限 100；超过则报错而不是把未收集文件当删除。
  - 同步状态记录 `last_scopes`，no-op 也记录，避免 scope 扩张被误判为已同步。
  - 同步前重新构建并比较本地 SHA，若用户在同步期间修改配置则中止。
  - `restore_history` 与 `sync_now` 共享单飞锁。
  - 历史列表响应 2 MB 上限、200 条上限。
  - 备份临时文件使用唯一 UID 后缀，避免并发覆盖。

### 4.2 数据与兼容

- 本地新增：`sync/sync-state.json`（设备 ID/名称、last_revision、last_scopes、last_error、tombstones/冲突状态）。
- 远端新增：`clash-verge-sync/manifest.json`、`payload.zip`（或加密包）、`history/`。
- 新增 `IVerge.sync_settings`；密码字段使用现有 `serialize_encrypted`。
- 现有备份 ZIP 格式保持兼容；新增 `custom-rules.yaml` 条目。
- 旧 `enable_auto_delay_detection`、旧规则、旧备份继续兼容。

### 4.3 上游合并注意

- `src-tauri/src/core/backup.rs`：备份现在包含 `custom-rules.yaml`，且移除 `sync_settings.password`；WebDAV 列表解析函数改为共享。上游若修改备份内容，需保留这两点。
- `src-tauri/src/core/sync/` 是新增模块，通常可直接保留；若上游修改 `WebDavClient`/`reqwest_dav` 版本，需同步 `sync/client.rs`。
- `src-tauri/src/cmd/webdav.rs`：保存 WebDAV 配置后刷新 `SyncManager`；上游若重构该命令，需要保留。
- `src-tauri/src/cmd/save_profile.rs` / `cmd/profile.rs` / `cmd/custom_rules.rs`：on-change 同步触发点；上游若新增写配置命令，评估是否需要 `SyncManager::notify_local_change()`。
- `src-tauri/src/utils/dirs.rs`：新增 sync 状态/staging/conflicts 路径；上游若改目录布局，需同步。
- 前端 `backup-viewer.tsx` 已把「云同步」并入；上游若重做备份 UI，需要把 `SyncViewer` 入口和摘要迁移进去。
- 手工 bindings：`SyncSettings`、`SyncStatus`、`SyncRunResult`、`SyncHistoryEntry`、`SyncConflictCopy` 及 5 个命令。上游后请用 `pnpm gen:contract` 重新生成。

---

## 5. 配套/行为变更

| 变更 | 说明 | 位置 |
|---|---|---|
| 默认测速 URL 改 HTTPS | `http://cp.cloudflare.com/generate_204` → `https://cp.cloudflare.com/generate_204` | `services/delay.ts`、`proxy-head.tsx`、`proxy-group-tools.tsx`、`groups-editor-viewer.tsx`、`current-proxy-card.tsx`、`settings` 占位符、`core/tray/latency.rs`、相关测试 |
| 订阅默认更新间隔 | 新远程订阅默认 1440 分钟；低于下限会被归一 | `config/profiles.rs`、`utils/resolve/scheme.rs`、`config/verge.rs` |
| 备份包含自定义规则 | 新备份 ZIP 含 `custom-rules.yaml`；恢复旧备份不受影响 | `core/backup.rs` |
| 备份不含同步密码 | 备份 ZIP 的 `verge.yaml` 移除 `sync_settings.password`，避免跨设备恢复解密失败 | `core/backup.rs` |
| 全局延迟设置迁移 | Misc 中的自动延迟检测/默认 URL/超时移入「订阅与节点」；旧字段保留 | `misc-viewer.tsx`、`subscription-settings-viewer.tsx` |
| 健康排序/过滤 | 健康状态参与延迟排序；显示过滤只影响 UI，不改 mihomo 配置 | `use-filter-sort.ts`、`use-render-list.ts` |
| 手动测速默认 HTTPS | 托盘测速、分组测速、首页/代理页默认值统一 | 同第一行 |

---

## 6. 新增文件清单（上游合并时通常直接保留）

### Rust

```text
src-tauri/src/enhance/custom_rules.rs
src-tauri/src/cmd/custom_rules.rs
src-tauri/src/core/node_health.rs
src-tauri/src/core/node_health_probe.rs
src-tauri/src/cmd/node_health.rs
src-tauri/src/core/sync/mod.rs
src-tauri/src/core/sync/apply.rs
src-tauri/src/core/sync/client.rs
src-tauri/src/core/sync/crypto.rs
src-tauri/src/core/sync/manifest.rs
src-tauri/src/core/sync/overrides.rs
src-tauri/src/core/sync/payload.rs
src-tauri/src/core/sync/records.rs
src-tauri/src/core/sync/rules.rs
src-tauri/src/core/sync/settings.rs
src-tauri/src/core/sync/state.rs
src-tauri/src/cmd/sync.rs
```

### 前端

```text
src/components/rule/quick-add-rule-dialog.tsx
src/components/rule/rule-meta.ts
src/components/profile/profile-update-conflict-dialog.tsx
src/components/proxy/health-badge.tsx
src/components/setting/setting-subscription.tsx
src/components/setting/setting-sync.tsx（已并入 Backup & Sync，当前已删除；若上游备份 UI 重做可重新引入）
src/components/setting/mods/subscription-settings-viewer.tsx
src/components/setting/mods/sync-viewer.tsx
src/components/setting/mods/sync-history-viewer.tsx
src/components/setting/mods/sync-helpers.ts
src/providers/node-health-context.ts
src/providers/node-health-provider.tsx
src/services/node-health.ts
src/services/profile-update-conflict.ts
src/utils/version.ts
```

### 版本与文档

```text
custom-version.json
docs/CUSTOM_CHANGELOG.md
```

---

## 7. 合并上游后的必做检查

1. **重新生成契约**
   ```bash
   pnpm gen:contract
   pnpm i18n:types
   ```
   当前环境 `pnpm gen:contract` 会因 Windows loader `0xC0000139` 失败；请在可运行环境执行并比对 `src/services/bindings.ts`。手工补齐的同步/健康类型必须与 Rust 的 serde/specta 定义一致。

2. **编译与测试**
   ```bash
   pnpm typecheck
   pnpm lint
   pnpm test
   cargo check --manifest-path src-tauri/Cargo.toml
   cargo test --manifest-path src-tauri/Cargo.toml --no-run
   cargo fmt --all --check
   git diff --check
   ```
   Windows 环境下 Rust 测试二进制可能因 `0xC0000139` 无法启动；需在 CI/干净环境执行实际测试。

3. **重点冲突文件**
   - `src-tauri/src/config/verge.rs`：新增结构、默认值、patch、registry。
   - `src-tauri/src/feat/effects.rs`：`verge_registry!` 必须覆盖所有 `IVerge` 字段。
   - `src-tauri/src/enhance/mod.rs`：自定义规则注入顺序。
   - `src-tauri/src/feat/profile.rs`：订阅更新预检、staged update、冲突解决、文件恢复。
   - `src-tauri/src/cmd/profile.rs`：sync_id 补全、on-change 同步、Timer 刷新。
   - `src-tauri/src/core/timer.rs`：update_only_current、启动更新、失败通知。
   - `src-tauri/src/core/backup.rs`：备份内容、密码移除、WebDAV 解析共享。
   - `src-tauri/src/ipc.rs` / `cmd/mod.rs`：新命令注册。
   - `src/services/bindings.ts`：手工同步类型/命令。
   - `src/pages/settings.tsx`：订阅与节点卡片、Backup & Sync 入口。
   - `src/components/setting/mods/backup-viewer.tsx`：同步摘要/入口。
   - `src/components/proxy/*`：健康徽标、过滤、排序。
   - `src/locales/en|zh/*`：新增文案。

4. **数据兼容**
   - `custom-rules.yaml`、`node-health.json`、`sync-state.json` 是本地用户数据；升级不能删除或重置。
   - 备份 ZIP 新增 `custom-rules.yaml`，移除 `sync_settings.password`。
   - 同步远端目录 `clash-verge-sync` 与备份目录 `clash-verge-rev-backup` 不得混用。
   - 旧 `r<uid>.yaml` 规则继续生效；旧 `enable_auto_delay_detection` 继续迁移。

---

## 8. 已知限制 / 后续工作

- **L3 内容探测的归属**：通过本地 mixed port 发起，规则/全局/直连模式下可能走当前实际路由，不保证一定经过刚选中的节点；逐节点可用性以 L2 `expected` 探测为准，L3 作为选中节点补充验证。
- **自动故障转移**：`NodeHealthSettings.auto_failover` 字段保留，但尚未实现切换逻辑和 UI；当前不暴露。
- **订阅信息**：`show_traffic` / `expiry_warning_days` 已接入订阅卡片/首页的流量显示和到期颜色提示，但到期警告弹窗尚未实现。
- **连接入口**：快速添加规则目前通过连接详情按钮触发，未增加表格行右键菜单。
- **Home 选中验证**：`verify_on_select` 目前在 Proxies 页触发，Home 当前代理卡片尚未接入。
- **Rust 测试运行**：当前 Windows 环境 `cargo test` 二进制加载报 `0xC0000139`，需在 CI/干净环境执行；`cargo test --no-run` 通过。
- **契约生成**：`pnpm gen:contract` 同样受 loader 问题影响，bindings 为手工补齐；上游合并后必须重新生成。

---

## 9. 定制版本记录

根目录 `Changelog.md` 与上游保持一致，不写入本 fork 的条目，避免上游发布新版本时无法比较版本。

### custom.1（上游基线 v2.5.9，2026-10-20）

- 新增「规则」页和连接详情的快速添加规则，自定义规则独立保存并在订阅更新前校验失效分组。
- 新增「订阅与节点」设置：定时测速、真实站点健康检查、隐藏超时或错误节点。
- 新增「备份与同步」：WebDAV 定时同步订阅信息、脚本和自定义规则，冲突时保留历史版本。
- 优化节点测速默认使用 HTTPS，并在切换节点后提示真实站点访问失败。
- 增加 `custom-version.json` 与 `v2.5.9-custom.1` 展示，不修改上游版本号。
