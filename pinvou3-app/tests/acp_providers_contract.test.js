// ACP Provider（第三方中转）管理契约测试。
//
// 静态读源码断言安全与行为不变式：key 永不落明文 JSON / 日志、原子写 + 备份、
// 回退只删受管键、8 条命令全部注册、store 仅存 CredentialReference。
// 照 codex_acp_platform_contract.test.js 模式：node 直跑，退出码即结果。

'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const ROOT = path.resolve(__dirname, '..');
const TAU = path.join(ROOT, 'src-tauri', 'src');
const PROVIDERS = path.join(TAU, 'features', 'codex_acp', 'providers');
// Wave-2 拆分后 codex_acp 的职责分散在 mod.rs 与 install/login/introspect 等
// 子模块。契约检查的是「codex_acp 模块整体」的行为不变式,故 MOD 拼接整个
// 目录的源码;未拆分时(仅 mod.rs)行为不变。
const MOD = fs
  .readdirSync(path.join(TAU, 'features', 'codex_acp'))
  .filter((f) => f.endsWith('.rs'))
  .sort()
  .map((f) => fs.readFileSync(path.join(TAU, 'features', 'codex_acp', f), 'utf8'))
  .join('\n');
const PROVIDERS_MOD = fs.readFileSync(path.join(PROVIDERS, 'mod.rs'), 'utf8');
const CLAUDE = fs.readFileSync(path.join(PROVIDERS, 'claude.rs'), 'utf8');
const CODEX = fs.readFileSync(path.join(PROVIDERS, 'codex.rs'), 'utf8');
const KIMI = fs.readFileSync(path.join(PROVIDERS, 'kimi.rs'), 'utf8');
const LIFECYCLE = fs.readFileSync(path.join(PROVIDERS, 'lifecycle.rs'), 'utf8');
const COMMANDS = fs.readFileSync(
  path.join(TAU, 'app', 'commands', 'acp_providers.rs'),
  'utf8'
);
const COMMANDS_MOD = fs.readFileSync(path.join(TAU, 'app', 'commands', 'mod.rs'), 'utf8');
const LIB = fs.readFileSync(path.join(TAU, 'lib.rs'), 'utf8');
const CREDENTIALS = fs.readFileSync(
  path.join(TAU, 'platform', 'credential_store.rs'),
  'utf8'
);
const STORE = fs.readFileSync(path.join(TAU, 'features', 'codex_acp', 'store.rs'), 'utf8');

// ---------------------------------------------------------------- 1. 命令注册

// 11 条 Provider 命令 + 1 条会话覆盖命令全部注册到 generate_handler!
const PROVIDER_COMMANDS = [
  'list_acp_providers',
  'save_acp_provider',
  'delete_acp_provider',
  'switch_acp_provider',
  'switch_acp_provider_official',
  'uninstall_acp_agent',
  'cancel_acp_agent_install',
  'logout_acp_agent',
  'get_acp_provider_key',
  'export_acp_providers',
  'import_acp_providers',
  'probe_acp_agent_models',
  'set_codex_acp_session_provider',
];
for (const name of PROVIDER_COMMANDS) {
  assert.ok(
    LIB.includes(`commands::acp_providers::${name}`),
    `lib.rs 未注册命令 ${name}`
  );
}
assert.ok(
  COMMANDS_MOD.includes('pub(crate) mod acp_providers;'),
  'commands/mod.rs 未声明 acp_providers 模块'
);
assert.ok(
  MOD.includes('mod providers;'),
  'codex_acp/mod.rs 未声明 providers 子模块'
);

// ---------------------------------------------------------------- 2. 安全底线

// store JSON 结构不得包含明文 key 字段名。旧断言带 JSON 引号（"api_key"）
// 对 Rust 源码永不命中，是空断言假绿（复审测试建议）——改为检查 Rust 字段
// 模式，且只锚定持久化结构（ProviderRecord 定义段 + 会话 store 文件；
// ProviderTarget 含明文 key 是 CLI 配置语义，不在此范围）。
{
  const record_def = PROVIDERS_MOD.slice(
    PROVIDERS_MOD.indexOf('pub struct ProviderRecord'),
    PROVIDERS_MOD.indexOf('pub struct ProviderRecord') + 800
  );
  assert.doesNotMatch(record_def, /api_key/, 'ProviderRecord 不得有明文 api_key 字段');
  assert.doesNotMatch(STORE, /api_key/, '会话 store 不得有明文 api_key 字段');
}
assert.ok(
  PROVIDERS_MOD.includes('credential: Option<CredentialReference>'),
  'ProviderRecord 只允许 CredentialReference'
);
assert.ok(
  CREDENTIALS.includes('const ACP_PROVIDER_KEY_SERVICE: &str = "pinvou3-acp-provider-key"'),
  'credential_store 缺少 pinvou3-acp-provider-key service'
);
assert.ok(
  CREDENTIALS.includes('pub fn for_acp_provider(agent: &str, provider_id: &str)'),
  'credential_store 缺少 for_acp_provider 构造器'
);

// 写入器不得把 key 写进日志
for (const file of [CLAUDE, CODEX, KIMI, PROVIDERS_MOD]) {
  assert.doesNotMatch(
    file,
    /(log!|eprintln!|println!)[^\n]*(api_key|AUTH_TOKEN|OPENAI_API_KEY|auth_token)/i,
    '日志不得出现 key 相关内容'
  );
}

// 原子写（公共助手在 providers/mod.rs）+ 一次性备份 + 拒绝覆盖不可解析文件
assert.ok(
  PROVIDERS_MOD.includes('with_extension("tmp")') &&
    PROVIDERS_MOD.includes('fs::rename'),
  '公共写入助手必须 .tmp + fs::rename 原子替换'
);
assert.ok(
  PROVIDERS_MOD.includes('pinvou3-bak'),
  '首次受管写入必须备份 .pinvou3-bak'
);
assert.ok(
  STORE.includes('json.tmp') && STORE.includes('fs::rename'),
  'store 必须 .tmp + fs::rename 原子写'
);
for (const file of [CLAUDE, CODEX, KIMI]) {
  assert.ok(file.includes('atomic_write'), `${file} 必须经原子写助手落盘`);
  assert.ok(file.includes('拒绝覆盖'), '不可解析文件必须拒绝覆盖');
}

// ---------------------------------------------------------------- 3. 写入器行为

// claude 回退只删三个 env 键
assert.ok(
  CLAUDE.includes('ANTHROPIC_BASE_URL') &&
    CLAUDE.includes('ANTHROPIC_AUTH_TOKEN') &&
    CLAUDE.includes('ANTHROPIC_MODEL'),
  'claude 写入器必须管理三个 env 键'
);
assert.ok(
  CLAUDE.includes('[ENV_BASE_URL, ENV_AUTH_TOKEN, ENV_MODEL]') &&
    CLAUDE.includes('env_obj.remove'),
  'claude 回退必须删除三个受管 env 键'
);

// codex 写入器：env_key 引用、无明文 key、pv- 前缀管理、auth 回退判定
assert.ok(CODEX.includes('env_key'), 'codex 写入器必须使用 env_key 引用');
assert.ok(CODEX.includes('OPENAI_API_KEY'), 'codex 必须固定 OPENAI_API_KEY env_key');
assert.ok(CODEX.includes('PROVIDER_ID_PREFIX'), 'codex 回退必须按 pv- 前缀清理');
assert.ok(
  CODEX.includes('codex_config_relay_env_key_present'),
  'codex 必须提供 relay env_key 判定（认证回退用）'
);
assert.ok(
  MOD.includes('codex_config_relay_env_key_present(&raw)'),
  'codex_authenticated 必须接入 config.toml 回退'
);

// kimi 写入器：产物结构必须通过官方校验（真实校验函数名）
assert.ok(
  KIMI.includes('kimi_runtime_config_ready'),
  'kimi 测试必须直接调用官方校验函数'
);
// Kimi 原生协议：type="kimi"（Kimi Code 官方文档专用类型）且仅 kimi agent 可用
assert.ok(
  KIMI.includes('ProviderWireApi::Kimi => "kimi"'),
  'kimi 写入器必须支持 type="kimi" 映射'
);
assert.ok(
  PROVIDERS_MOD.includes('ProviderWireApi::Kimi && agent != "kimi"'),
  'kimi 原生协议必须仅限 Kimi Agent（save 校验）'
);

// 受管 provider id 前缀
assert.ok(
  PROVIDERS_MOD.includes('pub(crate) const PROVIDER_ID_PREFIX: &str = "pv-"'),
  'Provider id 前缀必须为 pv-'
);

// ---------------------------------------------------------------- 4. 配置文件路径

for (const entry of [
  [CLAUDE, 'settings.json', 'claude 必须写 ~/.claude/settings.json'],
  [CODEX, 'config.toml', 'codex 必须写 ~/.codex/config.toml'],
  [KIMI, 'config.toml', 'kimi 必须写 ~/.kimi-code/config.toml'],
]) {
  assert.ok(entry[0].includes(entry[1]), entry[2]);
}

// ---------------------------------------------------------------- 5. 切换顺序与会话重启

assert.ok(
  MOD.includes('restart_agent_sessions(backend)'),
  '切换/删除当前 Provider 后必须重启该 Agent 运行中会话'
);
assert.ok(
  MOD.includes('invalidate_auth_cache(backend)'),
  '切换后必须刷新当前 Agent 的认证缓存'
);
assert.ok(
  MOD.includes('configure_codex_provider_env') &&
    MOD.includes('command.env("OPENAI_API_KEY"'),
  'spawn 时必须为 Codex 注入 OPENAI_API_KEY'
);
assert.ok(
  MOD.includes('session_provider_api_key'),
  '必须实现会话 provider 解析（会话 option > 全局 current）'
);
assert.ok(
  MOD.includes('set_acp_session_provider'),
  '必须实现会话级 Provider 覆盖'
);
assert.ok(
  STORE.includes('clear_acp_config_value'),
  'store 必须支持清除会话配置值（恢复会话官方登录）'
);

// 卸载：运行中会话拦截 + 官方卸载路径
assert.ok(
  LIFECYCLE.includes('brew') && LIFECYCLE.includes('npm'),
  '卸载必须覆盖 brew / npm 来源'
);
assert.ok(
  LIFECYCLE.includes('npm_executable()'),
  'npm 卸载命令必须使用解析后的 npm 完整路径（Windows 裸名 npm 会 program not found）'
);
assert.ok(
  MOD.includes('运行中的会话') && MOD.includes('cleanup'),
  '卸载必须带运行中会话拦截与可选清理'
);

// ---------------------------------------------------------------- 6. 前端结构契约（主链回归保护）
// UI smoke 只在 test:bridge-smoke 链（需 Chromium）；这里在主链静态断言关键
// 结构，避免 ProvidersSection 被改坏时 npm test 无感知。

const PROVIDERS_SECTION = fs.readFileSync(
  path.join(ROOT, 'src', 'features', 'settings', 'ProvidersSection.jsx'),
  'utf8'
);
const PROVIDER_FORM = fs.readFileSync(
  path.join(ROOT, 'src', 'features', 'settings', 'ProviderFormModal.jsx'),
  'utf8'
);
const I18N = fs.readFileSync(path.join(ROOT, 'src', 'shared', 'i18n.js'), 'utf8');
const HOME_SWITCHER = fs.readFileSync(
  path.join(ROOT, 'src', 'features', 'conversation', 'HomeModeSwitcher.jsx'),
  'utf8'
);
const CODEX_VIEW = fs.readFileSync(
  path.join(ROOT, 'src', 'features', 'codex', 'CodexAcpView.jsx'),
  'utf8'
);

// 卸载确认弹窗与清理默认不勾选必须保留（A7 关键交互）
assert.match(PROVIDERS_SECTION, /acp-uninstall-cleanup/, '卸载弹窗必须保留清理复选框');
assert.match(
  PROVIDERS_SECTION,
  /uninstallConfirm\.cleanup/,
  '清理复选框必须接入卸载参数'
);
// 删除确认与删除后自动恢复官方（A6）
assert.match(PROVIDERS_SECTION, /acp-provider-delete-confirm/, '删除确认弹窗必须保留');
// env 冲突警告条（A10）
assert.match(PROVIDERS_SECTION, /acp-providers-env-warning/, 'env 冲突警告条必须保留');
// 配置文件不可解析警告（L8）
assert.match(
  PROVIDERS_SECTION,
  /acp-providers-unreadable-warning/,
  '配置文件不可解析警告条必须保留'
);
// 导出含明文 key 强警告且不自动复制（M3）
assert.match(
  PROVIDERS_SECTION,
  /exportWarningTitle/,
  '导出必须带明文 key 警告'
);
assert.doesNotMatch(
  PROVIDERS_SECTION,
  /clipboard\.writeText/,
  '导出不得自动写入剪贴板（防误粘贴泄露）'
);
// 表单：模型不自动填（预设只填 base URL/协议）、显隐按钮走 i18n
assert.doesNotMatch(
  PROVIDER_FORM,
  /preset\.model\)\s+setModel|setModel\(preset\.model/,
  '选择预设不得自动填模型（模型由用户自行输入）'
);
assert.match(
  PROVIDER_FORM,
  /copy\.showKey|copy\.hideKey/,
  'key 显隐按钮必须走 i18n 文案'
);
// 空 key / 删 key 自绘二级确认弹窗（Tauri WebView2 下系统 window.confirm 不弹）
assert.match(
  PROVIDER_FORM,
  /acp-provider-form-confirm/,
  '空 key/删 key 必须使用自绘二级确认弹窗'
);
assert.doesNotMatch(
  PROVIDER_FORM,
  /window\.confirm\s*\(/,
  '表单不得调用 window.confirm（Tauri WebView2 下不弹）'
);
// 新建空 key 必须走 keep（允许保存后补 key），不得无条件 replace
assert.match(
  PROVIDER_FORM,
  /trimmedKey\s*\?\s*'replace'\s*:\s*'keep'/,
  '新建空 key 必须走 keep 而非 replace（否则后端误报「替换时 api_key 不能为空」）'
);
// key 回填加固：回填返回空串不得置 keyLoaded（防保存时把旧 key 静默删除）
assert.match(
  PROVIDER_FORM,
  /if \(key\) setKeyLoaded\(true\)/,
  '回填返回空串时不得置 keyLoaded（保存须走 keep 保留旧密钥）'
);
assert.strictEqual(
  (I18N.match(/apiKeyEmptyConfirm/g) || []).length,
  3,
  '空 key 确认文案必须覆盖 zh/en/ja 三语'
);
// 会话级 Provider 覆盖仅 Codex 展示（H1）
assert.match(
  CODEX_VIEW,
  /activeAgentId === 'codex' && Boolean\(activeId\)/,
  '会话 Provider 覆盖必须仅对 Codex 展示'
);
// 齿轮入口文案走 i18n（N2）
assert.doesNotMatch(
  HOME_SWITCHER,
  /aria-label="Provider 配置"/,
  '齿轮 aria-label 不得硬编码中文'
);
assert.match(
  HOME_SWITCHER,
  /copy\.providerSettings/,
  '齿轮文案必须走 i18n'
);
// 预设有数据 testid 契约
assert.match(PROVIDERS_SECTION, /acp-provider-add/, '新增 Provider 按钮必须保留');
// 切回官方按钮（Provider 生效时必须可见）
assert.match(
  PROVIDERS_SECTION,
  /acp-provider-switch-official/,
  '必须提供「切回官方」按钮（switch_acp_provider_official 的前端入口）'
);

// ---------------------------------------------------------------- 7. 状态可见性增强契约

// 后端：EffectiveConfig 含 entries；AcpProvidersView 透传 effective_entries
assert.ok(
  PROVIDERS_MOD.includes('pub entries: Vec<EffectiveEntry>'),
  'EffectiveConfig 必须含 entries 字段（生效配置展示）'
);
assert.ok(
  PROVIDERS_MOD.includes('pub effective_entries: Vec<EffectiveEntry>'),
  'AcpProvidersView 必须透传 effective_entries'
);
assert.ok(
  PROVIDERS_MOD.includes('pub struct EffectiveEntry'),
  'EffectiveEntry 必须存在（camelCase 序列化）'
);
// 三个 writer 的 effective() 都填充 entries
for (const [file, name] of [[CLAUDE, 'claude'], [CODEX, 'codex'], [KIMI, 'kimi']]) {
  assert.ok(file.includes('entries.push'), `${name} writer 必须填充生效配置 entries`);
}
// entries 填充代码不得出现凭据字段（防回归）
for (const file of [CLAUDE, CODEX, KIMI]) {
  assert.doesNotMatch(
    file,
    /entries\.push\([\s\S]{0,200}(AUTH_TOKEN|api_key|API_KEY)/,
    '生效配置 entries 不得包含凭据字段'
  );
}
// codex wire_api 固定 responses（M4：chat/anthropic 均不合法）
assert.ok(
  CODEX.includes('Value::String("responses".into())'),
  'codex writer 必须写 wire_api="responses"（官方唯一合法值）'
);
assert.doesNotMatch(
  CODEX,
  /"chat"\.into\(\)|"anthropic"\.into\(\)/,
  'codex writer 不得写 chat/anthropic wire_api'
);
// 前端：env 覆盖降格徽标 + 生效配置只读区 + N10 警告
assert.match(
  PROVIDERS_SECTION,
  /currentOverriddenByEnv/,
  'env 覆盖时必须展示降格徽标文案'
);
assert.match(
  PROVIDERS_SECTION,
  /acp-providers-effective/,
  '生效中配置只读区必须保留'
);
assert.match(
  PROVIDERS_SECTION,
  /effectiveEntries/,
  '前端必须消费 effectiveEntries'
);
assert.match(
  PROVIDER_FORM,
  /noAnthropicEndpointWarning/,
  'Claude 选无 Anthropic 端点预设时必须警告（N10）'
);
// codex/claude 表单 wire 锁定（M4）
assert.match(
  PROVIDER_FORM,
  /agent === 'claude' \|\| agent === 'codex'/,
  'wire 选择必须对 claude 与 codex 同时锁定'
);

// ---------------------------------------------------------------- 8. 安装失败诊断增强

// stderr 无有效内容时给出可操作提示（网络检查 + npm/官方脚本手动安装路径）
assert.ok(
  MOD.includes('请检查网络连接后重试'),
  '安装脚本失败且 stderr 无信息时必须给出可操作提示'
);
// 手动安装指引按 Agent 动态生成（不能一律指向 codex）：模板引用 {npm_pkg}
// 占位，且为 codex/claude/kimi 各自映射真实 npm 包名。
assert.ok(
  MOD.includes('npm install -g {npm_pkg}'),
  '提示必须含 npm 手动安装路径（按 Agent 动态生成 npm_pkg）'
);
assert.ok(
  MOD.includes('npm_package(backend).unwrap_or("")'),
  'npm 手动安装提示必须按 Agent 取包名，不得硬编码 codex'
);
for (const [backend, pkg] of [
  ['CodexAcp', '@openai/codex'],
  ['ClaudeAcp', '@anthropic-ai/claude-code'],
  ['KimiAcp', '@moonshot-ai/kimi-code'],
]) {
  assert.ok(
    MOD.includes(`${backend} => Some("${pkg}")`),
    `手动安装提示必须为 ${backend} 映射 npm 包名 ${pkg}`
  );
}
// 前端：安装 busy 按 agent 隔离 + 文案为「正在安装」而非「保存中」
assert.match(
  PROVIDERS_SECTION,
  /'install:' \+ activeAgent/,
  '安装 busy 必须按 Agent 隔离（避免跨标签页残留）'
);
assert.match(
  PROVIDERS_SECTION,
  /copy\.installing/,
  '安装按钮 busy 文案必须为「正在安装」'
);

// ---------------------------------------------------------------- 8.1 安装可靠性：进行中状态恢复 + 取消安装

// 后端：安装子进程 pid 注册表 + 取消标记 + 取消入口
assert.ok(
  MOD.includes('install_children') && MOD.includes('install_cancelled'),
  'mod.rs 必须实现安装子进程 pid 注册表与取消标记'
);
assert.ok(
  MOD.includes('pub async fn cancel_agent_install'),
  'mod.rs 必须实现 cancel_agent_install（按 pid 杀安装进程树）'
);
// 前端：安装中状态从 status 派生（设置页关闭重开不丢）+ 取消按钮
assert.match(
  PROVIDERS_SECTION,
  /status && status\.installing/,
  '安装中状态必须从 status.installing 派生（设置页关闭重开不丢）'
);
assert.match(
  PROVIDERS_SECTION,
  /acp-cli-install-cancel/,
  '安装中必须提供取消按钮（cancel_acp_agent_install 的前端入口）'
);
// 升级后版本有效性校验：命令 exit 0 不等于升级生效（官方脚本假成功 /
// npm allowScripts 拦截 postinstall 会让版本原地不动，必须如实报错）
assert.ok(
  MOD.includes('fn verify_upgrade_effective'),
  'mod.rs 必须实现升级后版本有效性校验（防「成功但版本未变」）'
);
assert.ok(
  MOD.includes('previous_version') && MOD.includes('previous_installed'),
  '升级校验必须基于升级前版本与安装态（探测失败时 previous 缺失不能当全新安装跳过）'
);
assert.ok(
  MOD.includes('action != "none"'),
  '未执行安装动作（none）时跳过升级校验'
);
assert.ok(
  MOD.includes('allow-scripts') && MOD.includes('npm approve-scripts'),
  '升级未生效的错误必须给出 allow-scripts 拦截的处理指引'
);
// 官方脚本假成功（Claude install 检测到已存在就跳过覆盖却报成功）：升级前
// 必须把旧二进制改名移开让脚本走全新安装路径，失败时恢复旧文件。
assert.ok(
  MOD.includes('fn move_official_binaries_aside'),
  'official_script 升级前必须把旧二进制改名移开（防 install 假成功）'
);
assert.ok(
  MOD.includes('pre-upgrade') && MOD.includes('restore_backups'),
  '备份必须用 pre-upgrade 后缀，脚本失败必须恢复旧文件'
);
// 安装进度跨窗口/App 重启恢复：status 必须携带 install_command/install_latest_line，
// 共享 store 在安装收口后清除（只保留进行中）。
assert.ok(
  MOD.includes('pub install_command: Option<String>')
    && MOD.includes('pub install_latest_line: Option<String>'),
  'status 必须携带安装进度字段（前端重挂载/重启后恢复「执行命令」行）'
);
assert.ok(
  MOD.includes('struct InstallProgressStore') && MOD.includes('fn clear_install_progress'),
  '安装进度必须走共享 store，且安装收口后清除'
);

// ---------------------------------------------------------------- 9. Claude 细化模型槽位 + env 生效值（改动 5）

// 槽位定义：mod.rs 提供 CLAUDE_MODEL_SLOTS（opus/sonnet/haiku/fable/subagent → env 键）
assert.ok(
  PROVIDERS_MOD.includes('pub(crate) const CLAUDE_MODEL_SLOTS'),
  'mod.rs 必须定义 CLAUDE_MODEL_SLOTS 槽位表'
);
for (const envName of [
  'ANTHROPIC_DEFAULT_OPUS_MODEL',
  'ANTHROPIC_DEFAULT_SONNET_MODEL',
  'ANTHROPIC_DEFAULT_HAIKU_MODEL',
  'ANTHROPIC_DEFAULT_FABLE_MODEL',
  'CLAUDE_CODE_SUBAGENT_MODEL',
]) {
  assert.ok(PROVIDERS_MOD.includes(envName), `槽位表必须包含 ${envName}`);
}
// ProviderRecord/ProviderTarget 携带 model_slots；claude 必填校验
assert.ok(
  PROVIDERS_MOD.includes('pub model_slots: Option<std::collections::BTreeMap<String, String>>'),
  'ProviderRecord/ProviderTarget 必须携带 model_slots'
);
assert.ok(
  PROVIDERS_MOD.includes('模型为必填项'),
  'claude 保存时槽位必须必填校验（缺省会走官方流量）'
);
// claude writer：apply 写槽位、revert 删槽位
assert.ok(
  CLAUDE.includes('CLAUDE_MODEL_SLOTS'),
  'claude writer 必须使用槽位表写入/清理'
);
// env 生效值（改动 5）：env_var_specs 区分凭据；凭据值不回传
assert.ok(
  PROVIDERS_MOD.includes('fn env_var_specs') && PROVIDERS_MOD.includes('env_effective_entries'),
  '必须提供 env_var_specs 与 env_effective_entries（env 生效值展示）'
);
assert.ok(
  PROVIDERS_MOD.includes('("ANTHROPIC_AUTH_TOKEN", true)'),
  '凭据类 env 变量必须标记 secret=true'
);
assert.ok(
  PROVIDERS_MOD.includes('pub secret: bool'),
  'EffectiveEntry 必须含 secret 标志'
);
// 前端：槽位表单（仅 claude）+ env 掩码渲染
assert.match(PROVIDER_FORM, /CLAUDE_MODEL_SLOT_IDS/, '表单必须渲染细化模型槽位');
assert.match(PROVIDER_FORM, /modelSlotsRequired/, '表单必须校验槽位必填');
assert.match(PROVIDER_FORM, /changeModel/, '主模型变更必须联动槽位自动填充');
assert.match(PROVIDERS_SECTION, /envEffectiveEntries/, '前端必须消费 envEffectiveEntries');
assert.match(PROVIDERS_SECTION, /copy\.secretSet/, '凭据类 env 值必须渲染掩码文案');

// 1M 变体：按厂商归属（预设 models1m 字段）、统一小写 [1m]、Kimi Code 仅 k3[1m]
const CATALOG = fs.readFileSync(
  path.join(ROOT, 'src', 'features', 'settings', 'acp-provider-catalog.js'),
  'utf8'
);
assert.doesNotMatch(CATALOG, /\[1M\]/, '1M 变体必须统一小写 [1m]');
assert.ok(
  CATALOG.includes("models1m: ['k3[1m]']"),
  'Kimi Code 预设的 1M 变体必须仅 k3[1m]'
);
assert.ok(
  PROVIDER_FORM.includes('activePreset.models1m'),
  '选中预设时 1M 变体必须按厂商过滤'
);
// 候选全量展示（不做字符过滤，仅匹配排序），不用原生 datalist
assert.ok(
  PROVIDER_FORM.includes('ModelSuggestInput'),
  '模型输入必须使用全量展示的建议组件（不得用会过滤候选的原生 datalist）'
);
assert.doesNotMatch(
  PROVIDER_FORM,
  /<datalist/,
  '不得使用原生 datalist（字符过滤会隐藏候选，造成「只有几个模型可选」的误解）'
);

// 切换/删除 Provider 后必须重写草稿配置快照（否则对话页模型显示旧 Provider；
// 直接删除快照会让选择器整排消失——必须 reseed 而非仅失效）
assert.match(
  PROVIDERS_SECTION,
  /reseedDraftControlsAfterProviderSwitch/,
  '切换/删除 Provider 后必须重写草稿配置快照'
);
// 快照缓存必须独立成模块：ProvidersSection → CodexAcpView → SettingsView 的
// 循环引用会白屏
const DRAFT_CONTROLS_MODULE = fs.readFileSync(
  path.join(ROOT, 'src', 'features', 'codex', 'acp-draft-controls.js'),
  'utf8'
);
assert.match(
  DRAFT_CONTROLS_MODULE,
  /export function reseedDraftControlsAfterProviderSwitch/,
  'acp-draft-controls.js 必须导出草稿快照重写函数'
);
assert.match(
  DRAFT_CONTROLS_MODULE,
  /option\.id !== 'model'/,
  '重写快照必须剔除 config_options 里的旧 model 选项（否则仍显示旧模型名）'
);
assert.doesNotMatch(
  PROVIDERS_SECTION,
  /from ['"]\.\.\/codex\/CodexAcpView\.jsx['"]/,
  'ProvidersSection 不得 import CodexAcpView（循环引用会白屏）'
);

// 一次性模型探针：切换/删除 Provider 后写标记，对话页草稿态真实连接一次 ACP
// 拉取真实模型列表覆盖 reseed 占位，之后恢复懒加载（发消息才建会话的主行为不变）
assert.match(
  PROVIDERS_SECTION,
  /markAcpModelsProbePending/,
  '切换/删除 Provider 后必须写一次性模型探针标记'
);
assert.match(
  DRAFT_CONTROLS_MODULE,
  /export function markAcpModelsProbePending/,
  '探针标记助手必须收敛在 acp-draft-controls.js（防 key 两侧漂移）'
);
assert.match(
  DRAFT_CONTROLS_MODULE,
  /export function consumeAcpModelsProbePending/,
  '探针标记必须先清再探（一次性、防重入）'
);
assert.ok(
  MOD.includes('pub async fn probe_agent_model_options'),
  'mod.rs 必须实现一次性模型探针 probe_agent_model_options'
);
assert.match(
  MOD,
  /probe_agent_model_options[\s\S]*?self\.evict\(&probe_id\)/,
  '模型探针必须 evict 探针会话（关掉进程，防泄漏）'
);
assert.match(
  MOD,
  /probe_agent_model_options[\s\S]*?self\.agents\.remove\(&probe_id\)/,
  '模型探针必须删除 store 记录（防残留）'
);
assert.match(
  CODEX_VIEW,
  /probe_acp_agent_models/,
  'CodexAcpView 草稿态必须触发一次性模型探针'
);

// 官方登录/登出按钮（三个 Agent 均可登出：kimi 走 provider remove managed:kimi-code）
assert.match(PROVIDERS_SECTION, /acp-cli-login/, '必须提供官方登录按钮');
assert.match(PROVIDERS_SECTION, /acp-cli-logout/, '必须提供官方登出按钮');
assert.doesNotMatch(
  PROVIDERS_SECTION,
  /activeAgent !== 'kimi' &&/,
  '不得对 kimi 隐藏登出按钮（provider remove 已实现非交互登出）'
);
assert.ok(
  LIFECYCLE.includes('fn logout_args')
    && LIFECYCLE.includes('"auth", "logout"')
    && LIFECYCLE.includes('"provider", "remove", "managed:kimi-code"'),
  'lifecycle 必须提供三 Agent 的登出命令参数（kimi 走 provider remove）'
);
// 标签页切换不得残留上一个 agent 的内容：缓存按 agent 键控 + 异步写只落当前标签页
assert.match(
  PROVIDERS_SECTION,
  /setView\(null\);\s*\n?\s*setStatus\(null\);/,
  '切换 Agent 标签页且无缓存时必须立即清空旧 view/status（防残留）'
);
assert.match(
  PROVIDERS_SECTION,
  /activeAgentRef/,
  '异步加载必须仅对当前标签页回写（activeAgentRef 竞态保护）'
);
assert.match(
  PROVIDERS_SECTION,
  /for \(const agent of AGENTS\)/,
  '进入页面必须并行加载三个 Agent（不得按标签页惰性加载）'
);

// 别名标签仅 Claude 需要；kimi 中转激活时模型列表必须过滤掉官方模型（pv-* 前缀）
assert.match(
  CODEX_VIEW,
  /activeAgentId === 'claude' \? model\.id : undefined/,
  '别名标签必须仅 Claude 使用'
);
assert.match(
  CODEX_VIEW,
  /kimiRelayActive/,
  'kimi 中转激活时必须过滤模型列表（仅保留受管 pv-* 条目）'
);

// Anthropic 端点仅 claude 或 Anthropic 协议预设使用：不得以 wireLocked 判定
// （codex 也锁 wire，但走 OpenAI 兼容端点，否则会错填 api.deepseek.com/anthropic）。
// 锚定完整表达式（含 `|| preset.wireApi === 'anthropic'` 子句），避免子串
// 匹配假绿（复审测试建议）。
assert.match(
  PROVIDER_FORM,
  /useAnthropicEndpoint = agent === 'claude' \|\| preset\.wireApi === 'anthropic'/,
  'Anthropic 端点判定必须仅限 claude 或 Anthropic 协议预设（不得用 wireLocked）'
);

// codex 记录归一 openai + 徽标如实显示 Responses（不得误标 Anthropic 兼容）
assert.ok(
  PROVIDERS_MOD.includes('let wire_api = if agent == "codex"'),
  'codex 保存时 wire_api 必须归一为 openai（writer 固定 responses）'
);
assert.match(
  PROVIDERS_SECTION,
  /agent === 'codex'\s*\?\s*copy\.wireResponses/,
  'codex 的 wire 徽标必须显示 Responses'
);

// 安装/升级前必须停掉该 Agent 的运行中会话（防 Windows 二进制占用 EBUSY）
// 锚点用分派处的 `let result = match action`：action 解析（let action = match
// action）在 preflight 自检前已完成，restart 必须仍位于分派之前。
{
  const installFn = MOD.match(/pub async fn install_agent[\s\S]*?let result = match action/);
  assert.ok(installFn, 'install_agent body must exist');
  assert.ok(
    installFn[0].includes('restart_agent_sessions(backend)'),
    'install_agent 必须在分派前 shutdown 运行中会话（防二进制占用）'
  );
}

// codex 受管模型 catalog：apply 写 model_catalog_json + catalog 文件（消除
// 中转模型的 metadata 警告）；revert 只清理指向受管文件的键与文件
assert.ok(
  CODEX.includes('model_catalog_json') && CODEX.includes('pinvou3-model-catalog.json'),
  'codex writer 必须生成受管模型 catalog 并挂载 model_catalog_json'
);
assert.ok(
  CODEX.includes('write_model_catalog') && CODEX.includes('ends_with(CATALOG_FILE_NAME)'),
  'codex revert 必须只清理指向受管 catalog 的键（保留用户自配）'
);
// 上下文窗口（可选）：record/target/save 携带，codex 写 catalog、kimi 写
// max_context_size、表单仅 codex/kimi 展示
assert.ok(
  PROVIDERS_MOD.includes('pub context_window: Option<i64>'),
  'ProviderRecord/ProviderTarget 必须携带 context_window'
);
assert.ok(
  CODEX.includes('context_window.unwrap_or(CATALOG_DEFAULT_CONTEXT_WINDOW)'),
  'codex catalog 必须使用 Provider 的 context_window（默认兜底）'
);
assert.ok(
  KIMI.includes('context_window\n') || KIMI.includes('context_window\r\n') || KIMI.includes('.context_window.unwrap_or_else'),
  'kimi max_context_size 必须使用 Provider 的 context_window（默认兜底）'
);
assert.match(
  PROVIDER_FORM,
  /agent !== 'claude' && \(/,
  '上下文窗口字段必须仅对 codex/kimi 展示（claude 用 [1m] 变体）'
);

console.log('acp_providers_contract: OK (all invariants hold)');
