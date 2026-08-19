# 智能表格数据表模版索引

本文档汇总了所有可用的智能表格数据表模版，按业务场景分类整理。当用户需要从零开始创建智能表格时，可参考以下模版选择合适的表结构。

## 📋 模版分类概览

| 分类 | 适用场景 | 核心模版 | 参考文档 |
| --- | --- | --- | --- |
| **项目管理** | 项目全流程管理、任务跟踪、进度管控 | 任务管理、问题跟进、工单跟踪、立项申请 | [project_management.md](./project_management.md) |
| **团队任务** | 日常任务协作、工作计划、周会记录 | 待办清单、工作计划、周会、日报 | [team_tasks.md](./team_tasks.md) |
| **个人效率** | 个人任务和计划管理 | 待办清单、月度计划看板 | [personal_efficiency.md](./personal_efficiency.md) |
| **销售经营** | CRM客户管理、销售业绩分析、订单管理 | 销售CRM、业绩看板、会员管理 | [sales_and_operations.md](./sales_and_operations.md) |
| **人事行政** | 招聘、考勤、薪资、绩效考核 | OKR管理、员工信息、会议室预约 | [hr_and_administration.md](./hr_and_administration.md) |
| **办公必备** | 日常办公核心场景 | 费用报销、物品领用、信息收集 | [office_essentials.md](./office_essentials.md) |
| **AI提效** | 利用AI自动化处理高频工作 | AI日报总结、AI分析、AI文案生成 | [ai_efficiency.md](./ai_efficiency.md) |
| **链接应用** | 同步审批、考勤、收款等外部数据 | 审批仪表盘、考勤分析、经营收款 | [connect_to_app.md](./connect_to_app.md) |
| **产品研发-项目** | 研发项目全流程管理 | 需求池、人力甘特图 | [rd_process_project.md](./rd_process_project.md) |
| **产品研发-研发** | 研发过程管理 | 流程图、BUG跟进、走查问题 | [rd_process_research.md](./rd_process_research.md) |
| **产品研发-运维** | 运维工单和设备管理 | 运维问题、设备台账 | [rd_process_ops.md](./rd_process_ops.md) |
| **产品研发-客户** | 客户线索和售后管理 | 客户跟进、满意度调研 | [rd_process_customer.md](./rd_process_customer.md) |
| **微信客户** | 基于企微的客户服务跟进 | 客户服务、群运营、订单管理 | [wechat_customer.md](./wechat_customer.md) |
| **工作汇报** | 规范化工作汇报流程 | 日报周报、团队汇总 | [work_report.md](./work_report.md) |
| **生产制造** | 制造业生产全流程管理 | 生产日报、车间巡检、质检记录 | [manufacturing.md](./manufacturing.md) |
| **门店管理** | 连锁门店综合管理 | 巡店记录、问题反馈、库存管理 | [store_management.md](./store_management.md) |
| **财务会计** | 财务核心场景管理 | 财务报表、合同台账、发票管理 | [financial_accounting.md](./financial_accounting.md) |
| **采购物流** | 采购申请、供应商、物流跟踪 | 供应商管理、采购申请、询价比价 | [procurement_logistics.md](./procurement_logistics.md) |
| **市场营销** | 广告投放、营销活动、内容选题 | 投放管理、活动策划 | [marketing.md](./marketing.md) |
| **台账记录** | 通用台账管理场景 | 设备台账、退换货、发货明细 | [ledger_records.md](./ledger_records.md) |

## 🚀 快速选表指南

### 用户从零开始建表时的处理流程

1. **了解用户场景**：询问用户的业务场景是什么（如项目管理、销售跟进、人事行政等）
2. **匹配模版**：根据场景匹配上述分类中的模版文件，读取模版文件开头的「包含表格模版」列表，选择匹配的表格模版
3. **推荐对应模版**：推荐上一步匹配的核心模版
4. **提供表结构设计**：引导用户阅读参考文档中的字段定义和仪表盘配置
5. **协助创建实施**：使用 `references/smart-sheet-edit.md` 中的接口帮用户实际创建表结构

### 常见场景推荐

| 用户诉求 | 推荐模版分类 | 首选模版 |
| --- | --- | --- |
| "我要管理项目进度" | 项目管理 | 任务管理、通用项目管理 |
| "我要管理团队日常工作" | 团队任务 | 工作计划表、任务管理 |
| "我要做客户管理" | 销售经营 / 微信客户 | 销售CRM系统、客户跟进表 |
| "我要管理招聘流程" | 人事行政 | 招聘进度管理 |
| "我要做费用报销" | 办公必备 | 费用报销单、报销登记与审批 |
| "我想用AI自动化处理" | AI提效 | AI日报总结、AI分析类模版 |
| "我要管理生产质量" | 生产制造 | 车间巡检、质检记录 |
| "我要管理多个门店" | 门店管理 | 连锁门店任务/巡店管理 |

> **提示**：详细模版字段定义、仪表盘配置请参考各分类的完整文档。
