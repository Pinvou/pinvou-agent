// §9.9 文件夹通道 ensure 结果解释(纯逻辑):物化/锚定复用提取项目 id、排除
// 列表无 outcome、失败 outcome 与 IPC 错误的区分——三条通道(选择器浏览、
// chat/codex 最近目录)共用这份判定。
import assert from "node:assert/strict";
import test from "node:test";

import { interpretFolderEnsureOutcomes } from "../src/features/projects/folderEnsure.js";

test("created outcome carries the materialized project id", () => {
  const interpreted = interpretFolderEnsureOutcomes([
    { status: "created", project: { id: "p-new", name: "sub" } },
  ]);
  assert.equal(interpreted.materialized, true);
  assert.equal(interpreted.projectId, "p-new");
  assert.equal(interpreted.failed, false);
});

test("covered outcome reuses the anchored project id", () => {
  const interpreted = interpretFolderEnsureOutcomes([
    { status: "covered", project_id: "p-existing" },
  ]);
  assert.equal(interpreted.materialized, true);
  assert.equal(interpreted.projectId, "p-existing");
});

test("no outcome means the exclusion list skipped the folder", () => {
  const interpreted = interpretFolderEnsureOutcomes([]);
  assert.equal(interpreted.materialized, false);
  assert.equal(interpreted.projectId, null);
  assert.equal(interpreted.failed, false);
});

test("a failed outcome is not the exclusion list", () => {
  const interpreted = interpretFolderEnsureOutcomes([{ status: "failed" }]);
  assert.equal(interpreted.materialized, false);
  assert.equal(interpreted.failed, true);
});

test("non-array payloads degrade to the exclusion shape", () => {
  for (const payload of [null, undefined, "oops", 42]) {
    const interpreted = interpretFolderEnsureOutcomes(payload);
    assert.equal(interpreted.materialized, false);
    assert.equal(interpreted.projectId, null);
    assert.equal(interpreted.failed, false);
  }
});

test("a malformed created outcome without a project id is not materialized", () => {
  const interpreted = interpretFolderEnsureOutcomes([{ status: "created", project: {} }]);
  assert.equal(interpreted.materialized, true);
  assert.equal(interpreted.projectId, null);
});
