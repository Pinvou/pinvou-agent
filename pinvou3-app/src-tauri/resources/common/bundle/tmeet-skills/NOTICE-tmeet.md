# 第三方组件声明 — 腾讯会议官方技能(tmeet-skill)

本目录下的 `tmeet-skill/`(SKILL.md + references/)同步自腾讯会议官方开源仓库
**TencentCloud/tencentmeeting-cli**(https://github.com/TencentCloud/tencentmeeting-cli)
tag **v1.0.15** 的 `skills/tmeet-skill/`,按其 **MIT License** 分发。

上游仓库根目录 `LICENSE` 为腾讯版权声明的 MIT 许可;`skills/tmeet-skill/` 目录内
无单独 LICENSE 文件,故此处内联保留许可证文本:

```
Tencent is pleased to support the open source community by making tencentmeeting-cli available.

Copyright (C) 2026 Tencent.  All rights reserved.

tencentmeeting-cli is licensed under the MIT.


Terms of the MIT:
--------------------------------------------------------------------
Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

说明:

- 技能本体不随 npm 包分发(`@tencentcloud/tmeet` 包内不含 skills),更新方式为按
  上游对应 tag 同步 `skills/tmeet-skill/` 到本目录,保留本声明。
- 品悟按用户连接状态门控该 skill:仅在用户已连接 `tmeet` 且未禁用腾讯会议技能时
  释放到运行时技能目录。
- `tmeet` CLI(`@tencentcloud/tmeet`)不随包内置,由
  `pinvou3-app/src-tauri/src/features/connectors/tmeet.rs` 的 npm 钉扎
  (`TMEET_NPM_SPEC`,当前 `@tencentcloud/tmeet@1.0.15`)在线安装;SKILL.md 中的
  `npm install -g @tencentcloud/tmeet@latest` 表述保持上游原样,实际版本以 Rust
  层钉扎为准。

## Pinvou3 本地修改登记

无本地修改。当前 `tmeet-skill/` 与上游 tag v1.0.15 的 `skills/tmeet-skill/`
逐字节一致(同步前亦与 v1.0.13 基线逐字节一致,无历史漂移)。
