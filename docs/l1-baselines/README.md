# L1 Judge Baselines

锚定历史 L1 跑结果作为质量参照系。改了 INSTRUCTIONS_MD / bridge / 模型 / system-reminder 后,新跑跟最近 baseline diff 看质量漂移。

## 目录结构

```
docs/l1-baselines/
├── README.md (本文件)
└── <version>-<date>/                  ← 一次 baseline
    ├── <scenario>.md × N              ← harness 落的 transcript
    └── judge-report.md                ← Claude 按 rubric 评分报告
```

## 已有 baseline

| 版本 | 日期 | scenario 数 | 总均分 | 说明 |
|---|---|---|---|---|
| `v0.8.37` | 2026-05-18 | 5 | 4.75 | 首次 baseline,Qwen3.6 + L1.5 工具表 + INSTRUCTIONS_MD v0.8.37 |

## 怎么用

### 1. 锚一份新 baseline

```bash
cargo test --test l1_dialog_harness -- --ignored --test-threads=1
# 拿到新 ts (target/l1-runs/<ts>/)
# 跟 Claude 说: "评一下 target/l1-runs/<ts>"
# 拿到 judge report 后:
ts=<ts>
ver=<version-date>
mkdir -p docs/l1-baselines/$ver
cp pinvou3-app/src-tauri/target/l1-runs/$ts/*.md docs/l1-baselines/$ver/
cp pinvou3-app/src-tauri/target/l1-judge/$ts-report.md docs/l1-baselines/$ver/judge-report.md
# 更新本 README 表格
git add docs/l1-baselines/$ver/ docs/l1-baselines/README.md
git commit -m "锚 L1 baseline $ver"
```

### 2. 跟历史 baseline diff

```
跟 Claude 说: "对比 docs/l1-baselines/v0.8.37/ 跟 target/l1-runs/<新ts>/"
```

Claude 读两边 judge-report 的总览表 + 关注 ±0.5 以上的维度变化,**diff 报告告诉哪些维度漂了、可能原因、是否要 rollback**。

### 3. 锚 baseline 的时机

- 每个 release tag 前 (release-v0.8.37 / release-v0.9.0 / ...)
- 改 INSTRUCTIONS_MD 大块内容前/后
- 升级 vLLM 或 Qwen 模型版本前/后
- 改 system-reminder 文案前/后

平时改代码不需要锚——只在"可能影响 LLM 输出质量"的改动前/后锚。

## 注意

- `target/l1-runs/` 跟 `target/l1-judge/` 在 `.gitignore` 内(cargo build 产物目录),不可作为长期参照
- `docs/l1-baselines/` 进 git,跨 worktree / 团队 / 时间维度可看
- rubric 改版本时,旧 baseline 的 judge-report 用的是旧 rubric,diff 要谨慎(rubric v1 vs v2 同个 4.5 分意思可能不同)
