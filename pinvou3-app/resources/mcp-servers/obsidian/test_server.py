# -*- coding: utf-8 -*-
"""rename_note 修双链回归测试:各 wikilink/md-link 形式都改对,且不误伤
同前缀([[notebook]])/同后缀([](my_note.md))的别的笔记。纯本地文件、无网络。
pytest 或 `python test_server.py` 均可。"""
import importlib.util
import os
import tempfile

_spec = importlib.util.spec_from_file_location(
    "obs_server", os.path.join(os.path.dirname(__file__), "server.py")
)
_obs = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_obs)


def _setup_vault(d):
    """note(被改名) + my_note/notebook(陷阱:同后缀/同前缀的别的笔记) + 一篇引用页。"""
    os.makedirs(os.path.join(d, "sub"), exist_ok=True)
    for n in ("note", "my_note", "notebook"):
        with open(os.path.join(d, n + ".md"), "w", encoding="utf-8") as f:
            f.write("# " + n + "\n")
    ref = "\n".join([
        "[[note]]",
        "[[note|别名]]",
        "[[note#标题]]",
        "![[note]]",
        "[[notebook]]",       # 前缀相同的别的笔记,不应改
        "[](note.md)",
        "[](my_note.md)",     # 后缀相同的别的笔记,不应改(原 bug 会改坏)
        "[](sub/note.md)",
    ])
    with open(os.path.join(d, "ref.md"), "w", encoding="utf-8") as f:
        f.write(ref)


def test_rename_fixes_links_without_collateral():
    saved = os.environ.get("OBSIDIAN_VAULT_PATH")
    with tempfile.TemporaryDirectory() as d:
        _setup_vault(d)
        os.environ["OBSIDIAN_VAULT_PATH"] = d
        try:
            ref = os.path.join(d, "ref.md")
            # 1) 预览(confirm=False)不动文件
            prev = _obs.rename_note("note", "renamed", confirm=False)
            assert prev["type"] == "confirm_required", prev
            assert "[[note]]" in open(ref, encoding="utf-8").read()

            # 2) 执行
            res = _obs.rename_note("note", "renamed", confirm=True)
            assert res["type"] == "obsidian_renamed", res
            out = open(ref, encoding="utf-8").read()

            # 应改的各形式
            for s in ("[[renamed]]", "[[renamed|别名]]", "[[renamed#标题]]",
                      "![[renamed]]", "[](renamed.md)", "[](sub/renamed.md)"):
                assert s in out, "应改未改: %s\n%s" % (s, out)
            # 不应误伤(回归核心)
            for s in ("[[notebook]]", "[](my_note.md)"):
                assert s in out, "被误改: %s\n%s" % (s, out)
            assert "my_renamed" not in out, "同后缀笔记被改坏:\n%s" % out

            # 文件真的改名
            assert os.path.isfile(os.path.join(d, "renamed.md"))
            assert not os.path.exists(os.path.join(d, "note.md"))
        finally:
            if saved is None:
                os.environ.pop("OBSIDIAN_VAULT_PATH", None)
            else:
                os.environ["OBSIDIAN_VAULT_PATH"] = saved


if __name__ == "__main__":
    test_rename_fixes_links_without_collateral()
    print("OK: rename_note 修双链回归通过")
