const PERSONAL_WORKBENCH_SCENE_KEY = 'personal-workbench';
/**
 * No static importer: tests/personal_workbench_scene_logic.test.js reads this
 * file as text, strips the export, and evaluates it by name in a Node vm
 * sandbox; knip cannot build an edge for that channel, so the `@public` tag
 * keeps it from being removed as a dead export.
 * @public
 */
export const PERSONAL_WORKBENCH_SCENE_ID = 39;
/**
 * Same as above: consumed by name through the vm sandbox in
 * tests/personal_workbench_scene_logic.test.js; knip cannot see that channel,
 * so the `@public` tag keeps it from being removed as a dead export.
 * @public
 */
export const PERSONAL_WORKBENCH_SCENE_NAME = '个人工作台';

const DEFAULT_PERSONAL_WORKBENCH_PROMPT = '你是 PINVOU 的个人数字工作台搭建专家。用户已经选择「个人工作台」场景，用户输入可能很短，例如“运动”“理财”“求职”“学习”。请把用户输入理解为要围绕该主题生成一个个人可用的网页版工作台，而不是只回答建议。\n\n交付目标：\n生成一个可直接运行的单文件 HTML 工作台。所有 CSS、JavaScript、图标、示例数据和资源必须内联在同一个 HTML 文件中；不依赖后端、账号体系、外部 CDN、外部字体、外部图表库或外部图片；离线打开也可用。\n\n工作台默认规范：\n1. 每个工作台最多选择 3-4 个核心模块，不要一次堆太多功能；如果用户需求过大，先做核心模块跑通。\n2. 页面第一屏必须是可用工作台，不要做营销式首页。\n3. 顶部必须有「今天要处理」区域，自动列出逾期、今天该做、快到期的事项；昨天没完成的内容自动滚到今天，不凭空消失。\n4. 首次打开必须预置 3-5 条示例数据，其中至少 1 条体现逾期或待处理状态，并提供「清空示例数据」入口。\n5. 所有用户输入即时保存到 localStorage，刷新和关闭页面后不丢失；localStorage key 使用 pinvou_{工作台标识}_ 前缀，避免冲突。\n6. 首屏提供「导出 JSON 备份」和「导入恢复」按钮；导入不限条数；清空数据需要二次确认；记账/财务类额外提供 Excel 兼容 CSV 导出。\n7. 用户数据累计到 30 条时，在顶部给出温和备份提示。\n8. 图表使用内联 SVG 或原生 Canvas 手写；图标使用内联 SVG，不使用 emoji 作为主要图标。\n9. 移动端优先：窄屏单列，按钮点击区域至少 44x44px，输入框字号不小于 16px；PC 端充分利用宽屏空间，可使用侧栏或多栏布局。\n10. 财务/记账类遵循国内习惯：支出用红色，收入用绿色，货币使用 ¥，日期使用 YYYY-MM-DD 或 MM月DD日。\n\n视觉要求：\n采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线、一致图标体系和明确状态色；整体配色根据用户主题定制，避免廉价渐变、杂乱装饰和模板化堆卡；按钮、输入框、列表、图表、空状态、错误提示都要有完整样式；交互反馈明显但克制。\n\n主题模块选择规则：\n- 学习/考试/读书/背单词：优先做学习规划、进度打卡、错题/笔记、周报复盘。\n- 任务/工作/项目/效率：优先做今日任务、状态看板、优先级、周报日报或数据报表。\n- 内容/自媒体/副业：优先做选题管理、素材库、发布排期、数据复盘。\n- 宝宝/育儿/亲子：仅在用户自由输入明确提到时兜底支持，优先做孩子模式、家长模式、任务奖励、成长记录；儿童向按钮至少 50px。\n- 生活/记账/健康/运动/习惯：优先做记账理财、习惯打卡、减脂健身、日程待办。\n- 求职：优先做投递看板、面试日程、公司/岗位记录、复盘笔记。\n- 旅行：优先做行程日历、地点清单、预算、物品清单。\n\n如果用户输入非常简短，请直接按最佳实践生成，不要反复追问；最多 1 个必要澄清问题，否则直接开做。';

const PERSONAL_WORKBENCH_TEMPLATES = [
  {
    id: 'life-log',
    title: '生活记录',
    prompt: '请为个人日常生活场景制作一个「生活记录」工作台，以单文件 HTML 交付，所有 CSS、JavaScript、图标与资源全部内联，不依赖外部组件，手机和电脑都能稳定使用。\n\n工作台至少包含：今日生活指数；记账理财（收支记录、预算、月度对比、分类筛选、消费结构、Excel 兼容 CSV 导出）；习惯健康（用户可新增或删除习惯，支持勾选、计数、数值三种打卡方式和 30 天热力图）；减脂健身（体重体脂、7 日均线、BMI、目标进度、热量缺口、可自定义周计划）；日程统筹；待买清单；书影音收藏（状态、星级、短评、封面墙/列表、年度统计）。\n\n数据必须输入即保存到 localStorage，关闭页面后仍在；提供 JSON 导出备份与不限条数导入，新增满 20 条提醒备份，存储失败和数据损坏要明确提示并允许恢复；预置少量可清空的演示数据。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体配色要有温暖生活感，避免廉价渐变和杂乱装饰；按钮、输入框、列表、图表和空状态都要有完整样式；交互反馈要明显但克制；移动端优先，PC 端充分利用宽屏空间；页面第一屏就要呈现可用的核心工作台，不要做营销式首页。',
  },
  {
    id: 'personal-ledger',
    title: '个人账本',
    prompt: '请制作一个名为「个人账本」的离线个人工作台，用单文件 HTML/CSS/JS 实现，所有资源内联，不依赖 CDN、后端或在线图表库，直接双击即可运行。\n\n工作台需要包含：真实时薪计算器（综合到手月薪、发薪月数、工作成本、在场时间、通勤和加班，并展开公式）、10 秒记账（金额、分类、备注，保存后立刻换算成工作时间）、首页最近 4 笔记录、月度收入与固定/弹性支出总结、自由基金目标与安全垫、原生 SVG 存款曲线、通勤/加班/涨薪情景模拟。\n\n数据使用 localStorage 保存，支持 JSON 导入导出和安全清理示例数据。默认提供相对当前日期的演示数据并明确标记为示例。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体配色要清爽、专业、有轻微手账感，避免廉价渐变和杂乱装饰；加入统一内联 SVG 图标和存钱主题插画；桌面使用侧栏，手机使用底部 Tab，所有主要控件适合触屏操作；页面第一屏就要呈现可用的核心账本，不要做营销式首页。',
  },
  {
    id: 'study-plan',
    title: '学习计划',
    prompt: '帮我做一个「学习计划」工作台，专门管理有终点、有总量的学习目标，比如“背完 2000 个单词”或“读完一本 440 页的书”，不是每天喝水那种无限期习惯打卡。一个 HTML 文件实现全部功能，不连任何外部接口，也不需要我提供资料。\n\n每个目标包含名称、单位、总量、截止日、配色，以及两个选填字段：最容易拦住我的障碍和“如果它出现，我就……”的对策。每天自动计算今日建议量=剩余量÷剩余天数，打卡时预填但允许修改。每个目标显示完成率、连续打卡天数、预计完成日；预计完成日按近 7 天平均速度推算，与截止日比较后显示提前或拖后，样本不足时显示“暂无推算”，不要编数字。\n\n连续天数允许每周从周一开始自动使用 1 个休息日：本周第一次漏打卡不中断，第二次才断。昨日漏打卡的目标今天自动置顶，分别用温和黄卡或红卡解释休息日和滚入今日；逾期目标显示天数，并提供“立即打卡”和“调整计划”两个出口。卡片展示用户预案。一天可记录多次，支持选填分钟数和补记最近 6 天，补记带“补”标并按真实日期参与连续天数和推算。\n\n页面分四个 Tab：\n1. 今日：首屏显示新建目标按钮、今天要处理、可直接打卡的目标卡，列表底部再保留醒目的新建入口；支持编辑和二次确认删除。\n2. 看板：每个目标显示进度环、三个核心数字、预案和最近 10 条可删除记录；再用手写 SVG 绘制近 14 天投入分钟的分目标堆叠柱状图，未填分钟的记录不硬凑。\n3. 周报：按周一到周日汇总投入量、打卡天数和分钟数，附上周同口径环比；可查看历史周，并保存保持、问题、尝试、下周预案四栏；一键生成包含环比和四栏内容的周报文本。\n4. 我的：导出 JSON、覆盖确认后导入、旧版本迁移、清空示例、输入“清空”后清空全部，以及添加到手机主屏幕的三步说明。\n\n预置背英语单词、读《人类简史》、Python 入门课 3 个带示例标签的目标和几天记录，覆盖补记、休息日、连续漏打、障碍预案等状态。累计 20 条打卡记录后，页面顶部出现可直接导出的温和备份横幅。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体风格专注、干净、有执行感，使用冷白底、绿色成功、红色逾期、琥珀色休息日等清晰语义色；目标达成时有克制的彩带动效；图标全部使用内联 SVG，不用 emoji；手机底部四 Tab 和单列布局，平板顶部横向导航，电脑左侧固定边栏与多栏布局。所有输入即时保存到 localStorage。交付一个无外部库、资源全内联、断网可用的单文件 HTML。',
  },
  {
    id: 'task-board',
    title: '任务看板',
    prompt: '生成一个「任务看板」单页应用（HTML/CSS/JS），用于个人任务和小项目管理。\n\n要求：1）包含今日任务、待办、进行中、已完成、逾期提醒几个区域；2）支持新增任务、编辑标题/备注/优先级/截止日期、拖拽或按钮切换状态、删除任务；3）自动统计今日完成数、逾期数、各状态任务数和本周完成趋势；4）支持按优先级、截止日期、状态筛选；5）使用 localStorage 持久化全部数据，并提供 JSON 导入导出；6）预置少量可清空的示例任务；7）PC 端使用看板布局，移动端使用底部 Tab 或单列分组；8）视觉风格简洁、专注、适合每天反复使用；9）所有 CSS、JavaScript 和图标资源内联到一个 HTML 文件，离线可用。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体风格要有生产力工具感，状态列和优先级颜色清晰但不刺眼；按钮、输入框、列表、图表和空状态都要有完整样式；交互反馈要明显但克制；移动端优先，PC 端充分利用宽屏空间；页面第一屏就要呈现可用的核心看板，不要做营销式首页。',
  },
  {
    id: 'job-tracker',
    title: '求职管理',
    prompt: '生成一个「求职管理」单页应用（HTML/CSS/JS），用于跟踪个人求职投递和面试进展。\n\n要求：1）包含投递看板、面试日程、公司记录、岗位对比、复盘笔记几个模块；2）每条投递记录包含公司、岗位、城市/远程、薪资范围、投递渠道、当前阶段、截止/面试时间、联系人、备注；3）支持新增、编辑、删除记录，并按阶段在“待投递、已投递、笔试/面试、Offer、已拒绝”之间切换；4）自动统计投递总数、面试率、Offer 数、平均反馈周期；5）支持按阶段、城市、薪资、日期筛选；6）使用 localStorage 保存数据，支持 JSON 导入导出；7）预置少量示例数据且可一键清空；8）PC 端适合横向看板和表格对比，移动端适合卡片列表；9）界面专业、冷静、信息密度适中；10）所有资源内联到单个 HTML 文件，离线可用。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体风格接近专业 CRM / 表格工具，信息密度适中，状态标签、时间线、面试提醒要醒目但克制；按钮、输入框、列表、图表和空状态都要有完整样式；移动端优先，PC 端充分利用宽屏空间；页面第一屏就要呈现可用的核心求职看板，不要做营销式首页。',
  },
  {
    id: 'travel-planner',
    title: '旅行计划',
    prompt: '生成一个「旅行计划」单页应用（HTML/CSS/JS），用于规划个人或家庭旅行。\n\n要求：1）包含行程日历、地点清单、预算管理、交通住宿、物品清单、旅行备忘几个模块；2）支持按天添加景点、餐厅、交通、酒店和自由活动，并可编辑时间、地点、费用、备注；3）自动统计总预算、已安排天数、每日花费、待预订事项和未完成物品；4）支持行程按日期分组展示，移动端可以像时间线一样浏览；5）使用 localStorage 持久化数据，并提供 JSON 导入导出；6）预置一组可清空的示例旅行计划；7）PC 端使用日程 + 预算 + 清单的多栏布局，移动端使用底部 Tab；8）视觉风格轻松、清爽、有旅行感，但不要做成营销落地页；9）所有 CSS、JavaScript、图标和示例数据内联到一个 HTML 文件，离线可用。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体风格要有清爽旅行手账感，地图感、日期轴、预算卡片和物品清单要容易浏览；按钮、输入框、列表、图表和空状态都要有完整样式；移动端优先，PC 端充分利用宽屏空间；页面第一屏就要呈现可用的核心旅行计划，不要做营销式首页。',
  },
  {
    id: 'fitness-checkin',
    title: '运动打卡',
    prompt: '生成一个「运动打卡」单页应用（HTML/CSS/JS），用于记录运动、训练计划和身体状态，支持 PC / 移动端响应式。\n\n要求：1）多页面结构或多 Tab 结构（首页、打卡页、数据看板、历史记录）；2）包含今日打卡、训练计划、运动记录、体重趋势、习惯连续天数模块；3）支持添加运动类型、时长、消耗、备注和日期；4）自动统计本周运动次数、总时长、连续打卡天数；5）使用 localStorage 存储数据，优先使用原生 Canvas 或 SVG 展示运动图表，不允许引用 CDN 或外部图表库；6）移动端底部 Tab 栏，PC 端侧边栏；7）所有资源内联到单个 HTML 文件，离线可用。\n\n视觉要求：采用现代 iOS / macOS 风格的高质感界面，使用圆角卡片、柔和阴影、清晰留白、细腻分割线和一致的图标体系；整体风格要有活力运动感，但不要过度游戏化，不要使用廉价赛博朋克堆叠效果；按钮、输入框、列表、图表和空状态都要有完整样式；交互反馈要明显但克制；移动端优先，PC 端充分利用宽屏空间；页面第一屏就要呈现可用的核心运动打卡，不要做营销式首页。',
  },
];

function getPersonalWorkbenchTemplate(index) {
  if (!Number.isSafeInteger(index)) return null;
  return PERSONAL_WORKBENCH_TEMPLATES[index] || null;
}

function getPersonalWorkbenchTemplateById(templateId) {
  const id = String(templateId || '').trim();
  if (!id) return null;
  return PERSONAL_WORKBENCH_TEMPLATES.find(template => template.id === id) || null;
}

function findPersonalWorkbenchTemplateDraft(userText) {
  const draft = String(userText || '').trim();
  if (!draft) return null;
  for (let index = 0; index < PERSONAL_WORKBENCH_TEMPLATES.length; index += 1) {
    const template = PERSONAL_WORKBENCH_TEMPLATES[index];
    const prompt = String(template.prompt || '').trim();
    if (prompt && draft.startsWith(prompt)) return { index, template };
  }
  return null;
}

function isPersonalWorkbenchTemplateDraftForTemplate(userText, template) {
  const draft = String(userText || '').trim();
  const prompt = String(template && template.prompt || '').trim();
  const title = String(template && template.title || '').trim();
  if (!draft || !prompt) return false;
  if (draft.startsWith(prompt) || prompt.startsWith(draft)) return true;
  if (title && draft.includes(`「${title}」`)) return true;
  return false;
}

function buildDefaultPersonalWorkbenchPayloadText(userText) {
  const text = String(userText || '').trim();
  if (!text) return '';
  return `${DEFAULT_PERSONAL_WORKBENCH_PROMPT}\n\n用户需求：\n${text}`;
}

function createPersonalWorkbenchMessageMeta(userText = '', templateRef = null) {
  const template = typeof templateRef === 'string'
    ? getPersonalWorkbenchTemplateById(templateRef)
    : getPersonalWorkbenchTemplate(templateRef);
  const meta = {
    pinvouScene: `work:${PERSONAL_WORKBENCH_SCENE_KEY}`,
    pinvouTemplateId: template ? template.id : undefined,
    pinvouTemplateTitle: template ? template.title : undefined,
  };
  if (!template) {
    const payloadText = buildDefaultPersonalWorkbenchPayloadText(userText);
    if (payloadText) meta.pinvouPayloadText = payloadText;
  }
  return meta;
}

function shouldUsePersonalWorkbenchScene(mode, subtab) {
  return mode === 'work' && subtab === PERSONAL_WORKBENCH_SCENE_KEY;
}

export {
  PERSONAL_WORKBENCH_SCENE_KEY,
  PERSONAL_WORKBENCH_TEMPLATES,
  DEFAULT_PERSONAL_WORKBENCH_PROMPT,
  buildDefaultPersonalWorkbenchPayloadText,
  createPersonalWorkbenchMessageMeta,
  findPersonalWorkbenchTemplateDraft,
  getPersonalWorkbenchTemplate,
  getPersonalWorkbenchTemplateById,
  isPersonalWorkbenchTemplateDraftForTemplate,
  shouldUsePersonalWorkbenchScene,
};
