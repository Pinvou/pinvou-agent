//! 项目层(Project Layer):会话的**逻辑归属**与**物理执行**解耦。
//!
//! - 逻辑层(本模块)只存人的判断:项目定义(id/名称/roots/排序位)与会话
//!   归属映射。整理类操作(移动归属/删除项目)只写本层。
//! - 物理层(`features/sessions`、`features/codex_acp`)存机器事实:会话的
//!   工作目录绑定创建后不可变,本层永不触碰。
//! - 依赖单向:本模块不依赖任何其它 feature(依赖方向 `app → features →
//!   platform/core`);跨 store 组合与会话存在性校验一律放在命令层。
//! - 删除项目永不删除会话:全体成员(显式归属 + 自动归组)写成显式移出,
//!   留在未分组且不随该文件夹的下一次自动物化复活;之后在该文件夹新建的
//!   会话没有归属条目,照常自动归组(等价 Codex `threads.project_id ...
//!   ON DELETE SET NULL` 之上叠加"移出条目压制复活"的语义)。
//!
//! 分组解析的确定性顺序由前端实现,本模块只提供数据:
//! ① 显式归属(assignments 命中;null = 显式移出,阻止自动归组复活)
//! ② 会话 workspace 落入项目 root → 自动归组
//! ③ 隐式文件夹分组 / 临时会话沉底(现状兜底)

mod store;
#[cfg(test)]
mod tests;

pub use store::{
    DeleteProjectReport, EnsureFolderOutcome, MoveSessionOutcome, Project, ProjectStore,
    SessionAssignments, removed_roots,
};
