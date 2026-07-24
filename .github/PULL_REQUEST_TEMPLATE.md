## 做什么

<!-- 1–2 句:这个 PR 解决什么、为什么 -->

## 自查(提交前过一遍)

- [ ] 跑了 `./scripts/fork-guard.sh --fast`(CI fast-gate 跑的就是它)
- [ ] 改了 `CodeWhale`(fork) submodule? → 同 PR 带了 `docs/fork-modifications.md` 条目 + `scripts/fork-guard.sh` 指纹(gitlink 焊在 `pinvou3-clean` 上,非游离 PR 分支)
- [ ] 创建 PR 时已基于最新 main；审批和 CI 通过后交由 Merge Queue 合入
- [ ] 加了测试或本地验证 —— 说明怎么验的:

## 备注

<!-- 风险、未决、需 reviewer 重点看的地方 -->
