#!/usr/bin/env python3
"""
pinvou3 文件管理大师 MCP server — 本地文件查找 / 磁盘占用扫描 / 回收站删除
（零第三方依赖，纯 stdlib）。

用法：由 CodeWhale MCP client 通过 stdio 启动（工具市场安装后注册进 mcp.json，
以 `python server.py` 拉起）。

协议：newline-delimited JSON-RPC 2.0 over stdio（与 weather server 同骨架）。
LLM 可见工具名：
  mcp_file_master_file_find          按文件名/目录名搜索本机文件（多词 AND + 相关度排序，支持过滤）
  mcp_file_master_disk_scan          只读扫描磁盘常见积聚地（含非系统盘），按 🟢🟡🔴 三级呈现
  mcp_file_master_file_trash         把文件/目录移入系统回收站（后台异步执行，绝不物理删除）
  mcp_file_master_file_trash_status  查询异步删除任务的进度与结果
  mcp_file_master_file_empty_recycle 清空回收站（物理删除，须用户明确确认）
  mcp_file_master_file_erase         物理删除 _pinvou_filemaster_trash 兜底目录内容（仅限该区域，不可恢复）
  mcp_file_master_file_restore       按删除日志还原误删文件
"""
import datetime
import io
import json
import math
import os
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.parse

PLATFORM = __import__("sys").platform

# Windows 默认 stdout/stdin 编码为 GBK，MCP 协议要求 UTF-8
if PLATFORM == "win32":
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8")
    sys.stdin = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")

# ── 共享常量 ─────────────────────────────────────────────────────────────

FIND_TIME_BUDGET_SEC = 8.0
FIND_MAX_LIMIT = 50

# 隐私 / 高 churn 目录剪枝（目录名小写匹配；命中即不向下递归）
PRUNE_DIR_NAMES = {
    ".ssh", ".gnupg", ".env",
    ".git", ".svn", ".hg",
    "node_modules", "__pycache__", ".pytest_cache", ".mypy_cache",
    ".codex", ".workbuddy-ai", ".kimi-code", ".pinvou3",
    "$recycle.bin", "system volume information",
}


def _reparse_target_realpath(path, mode, rtag):
    """reparse point（junction 等）→ 目标真实路径（normcase）；否则 → None。

    Windows 的循环 junction（如 AppData\\Local\\Application Data → AppData\\Local）
    在 Python 里 is_symlink() 为 False、is_dir() 为 True，直接递归会无限循环、
    磁盘统计重复求和。遍历中遇到 reparse 目录时用真实路径判重防环：
    环的每一层路径字符串都不同（A\\Application Data、A\\Application Data\\
    Application Data…），只有 realpath 相同——normpath 判重无效，必须 realpath。
    symlink 由 S_ISLNK 自然排除（不判重，直接不递归，与历史行为一致）。
    非 Windows 上 st_reparse_tag 恒为 None，此函数恒返回 None，零开销。
    mode/rtag 来自缓存元组（避免重复 stat）。"""
    if not rtag or stat.S_ISLNK(mode):
        return None
    try:
        return os.path.normcase(os.path.realpath(path))
    except OSError:
        return None


# ── 目录内容缓存（mtime 快照失效）────────────────────────────────────────
# 缓存"目录的直接子项元数据"：同一目录在对话内被多次扫描（file_find 换词重试、
# disk_scan 下钻）时跳过 scandir+stat，4 秒搜索预算能覆盖更多目录、减少截断。
# 正确性：目录 mtime 只在直接子项增删改时更新（子目录内部变化更新子目录自己的
# mtime），所以"mtime 未变 → 直接子项未变"成立；变了自动重扫自愈。
# 粒度=单目录直接子项，不缓存递归结果，无索引一致性负担。
_dir_cache = {}
_dir_cache_lock = threading.Lock()
_DIR_CACHE_MAX = 20000  # 防内存膨胀；满则整体清空（下次遍历重建）


def _cached_scandir(path):
    """读取目录直接子项元数据（带 mtime 快照缓存）。
    返回 [(name, st_mode, st_size, st_mtime, st_reparse_tag), ...]；
    目录不可读/不存在返回 None（调用方按现状处理）。"""
    try:
        st = os.stat(path, follow_symlinks=False)
    except OSError:
        return None
    key = _norm(path)
    with _dir_cache_lock:
        cached = _dir_cache.get(key)
        if cached is not None and cached["mtime"] == st.st_mtime:
            return cached["entries"]
    try:
        with os.scandir(path) as it:
            raw = list(it)
    except OSError:
        return None
    entries = []
    for e in raw:
        try:
            es = e.stat(follow_symlinks=False)
        except (PermissionError, OSError):
            continue
        entries.append((e.name, es.st_mode, es.st_size, es.st_mtime,
                        getattr(es, "st_reparse_tag", None)))
    with _dir_cache_lock:
        if len(_dir_cache) >= _DIR_CACHE_MAX:
            _dir_cache.clear()
        _dir_cache[key] = {"mtime": st.st_mtime, "entries": entries}
    return entries


def _seed_visited(roots):
    """把遍历根的真实路径种入 visited：root 自身是 junction 时，防止第一轮
    完整重复枚举一遍子树（同一目录经两条路径访问，matches/统计会重复）。"""
    seen = set()
    for r in roots:
        try:
            seen.add(os.path.normcase(os.path.realpath(r)))
        except OSError:
            pass
    return seen

# 默认搜索的高概率目录（用户主目录下，存在才搜）
PRIORITY_DIR_NAMES = ["Desktop", "Documents", "Downloads", "Pictures", "Videos", "Music"]

RISK_LEGEND = {
    "green": "纯缓存/临时文件：删除安全，应用会自动重建",
    "yellow": "含用户数据：需用户逐项判断，不主动删除",
    "red": "程序本体/系统区域：不建议删除，请走系统卸载或 Windows 磁盘清理",
}

SCAN_GROUP_BUDGET_SEC = 8.0     # 单组扫描时间预算，超出则标记 estimated
SCAN_TOTAL_BUDGET_SEC = 60.0    # 全部组 + 大文件扫描的总预算
SCAN_MAX_DEPTH = 8              # 单组递归限深，超过标记 estimated
LARGE_FILE_MIN_BYTES = 500 * 1024 * 1024
LARGE_FILE_LIMIT = 10

TRASH_PREVIEW_SIZE_BUDGET_SEC = 3.0  # 预览时单项目录求和预算
# 回收站配额：默认 = 盘容量 5%。FOF_NOCONFIRMATION 下 Shell 对超配额对象会
# 静默物理删除且返回成功（日志误记 recycle-bin、无法恢复），故超配额/大小
# 不可靠的项一律改走 _pinvou_filemaster_trash 兜底（见 _recycle_quota / _execute_trash_item）。

TRASH_DIRNAME = "_pinvou_filemaster_trash"

# 异步删除任务：SHFileOperationW 同步阻塞可能远超底座 30 秒 execute_timeout，
# confirm=true 改由后台线程执行，主线程立即返回 task_id，结果用 file_trash_status 轮询。
TRASH_MAX_ACTIVE_TASKS = 4   # 并发上限，超出直接拒绝（模型可稍后重试）
TRASH_TASK_TTL_SEC = 3600    # 已完成任务的保留时长，之后惰性清理
TRASH_STATUS_MAX_LIST = 10   # file_trash_status 无 task_id 时的列表上限
TRASH_MAX_PATHS = 50         # file_trash / file_erase 单次 paths 条数上限（防输出爆炸）

# 输出保险丝：底座工具结果 12,000 字符硬上限（超出压缩丢尾部）。
# 留 25% 余量；超过即压缩（先去可省字段，再二分缩减条数）。
OUTPUT_BUDGET_CHARS = 9000


def _json_len(obj):
    return len(json.dumps(obj, ensure_ascii=False))


def human_size(n):
    """字节数 -> '12.3 GB' 形式。"""
    n = float(n)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or unit == "TB":
            if unit in ("B", "KB"):
                return "%d %s" % (int(n), unit)
            return "%.1f %s" % (n, unit)
        n /= 1024


def _fmt_mtime(ts):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(ts))


# ── file_find ────────────────────────────────────────────────────────────

# query 分词：空白与中英文标点分隔；多词 AND 全命中
QUERY_SPLIT_RE = re.compile(
    r"[\s，、。；：！？（）【】《》“”‘’.,;:_\-+&%#@=/'\"!?()\[\]{}]+")
QUERY_MAX_WORDS = 5


def _split_query(query):
    """query 分词：小写、按空白与中英文标点切分、去空、**去重保序**、
    丢弃长度 1 且非数字的词（如 "report x" 的 x，避免单字符 AND 误杀），
    取前 QUERY_MAX_WORDS 词。空结果由调用方回退为整串单词（保留历史子串行为）。"""
    raw = QUERY_SPLIT_RE.split(query.strip().lower())
    seen, words = set(), []
    for w in raw:
        if not w or (len(w) == 1 and not w.isdigit()) or w in seen:
            continue
        seen.add(w)
        words.append(w)
        if len(words) >= QUERY_MAX_WORDS:
            break
    return words


def _name_segments(name_lc):
    """name 按非字母数字切分成分段：英文多词名（report_final → [report, final]）；
    纯中文名无分隔符，整体为一段。用于全词/前缀判定。"""
    return [s for s in re.split(r"[^a-z0-9]+", name_lc) if s]


def _name_match_score(name_lc, words):
    """name 对 query 词列表的相关度评分；任一词不命中 → None（AND 语义）。
    每个词取最高模式分：全词 +100（某分段 == 词）> 前缀 +60 > 子串 +30；
    总分 = 各词得分之和。segments 只在 gate 通过后计算一次（多词时先用
    低成本子串 AND gate 短路，避免对每个名字都做分段）。"""
    if len(words) == 1:
        w = words[0]
        if w not in name_lc:
            return None
        segments = _name_segments(name_lc)
        if w in segments:
            return 100
        if any(s.startswith(w) for s in segments):
            return 60
        return 30
    if not all(w in name_lc for w in words):  # AND gate：先子串短路
        return None
    segments = _name_segments(name_lc)
    total = 0
    for w in words:
        if w in segments:
            total += 100
        elif any(s.startswith(w) for s in segments):
            total += 60
        else:
            total += 30  # gate 已保证子串命中
    return total


def _iter_search(words, roots, skip_roots, deadline, matches, match_cap, state,
                 filters=None, visited=None, prune_extra=None):
    """DFS 搜索 roots（跳过 skip_roots 与剪枝目录），命中写入 matches(dict 去重)。
    state 用于跨调用累计命中数；到达上限/超时即提前返回。
    words: _split_query 的分词结果（多词 AND）；命中带相关度评分（score）。
    filters: dict 可选 {"exts": set, "after": ts, "before": ts, "min_size": int,
    "max_size": int}——匹配后再按扩展名/时间/大小过滤（大小只作用于文件）。
    visited: reparse 目录真实路径集合（见 _seed_visited），防 junction 环与重复遍历。
    prune_extra: 额外排除的目录名集合（exclude_dirs 参数），并入静态剪枝表。"""
    skip = {os.path.normcase(os.path.normpath(r)) for r in skip_roots}
    f = filters or {}
    exts, after, before = f.get("exts"), f.get("after"), f.get("before")
    min_size, max_size = f.get("min_size"), f.get("max_size")
    prune = PRUNE_DIR_NAMES | (prune_extra or set())
    seen = visited if visited is not None else set()
    stack = list(reversed(roots))
    while stack:
        if len(matches) >= match_cap:
            state["hit_limit"] = True
            return
        if time.monotonic() > deadline:
            state["timed_out"] = True
            return
        current = stack.pop()
        entries = _cached_scandir(current)  # mtime 快照缓存：未变目录跳过 scandir+stat
        if entries is None:
            continue
        for name, mode, size, mtime, rtag in entries:
            name_lc = name.lower()
            entry_path = os.path.join(current, name)
            if stat.S_ISDIR(mode):
                if name_lc in prune:
                    continue
                rp = _reparse_target_realpath(entry_path, mode, rtag)
                if rp is not None:
                    if rp in seen:
                        continue
                    seen.add(rp)
                norm = os.path.normcase(os.path.normpath(entry_path))
                if norm in skip:
                    continue
                stack.append(entry_path)
            if words:
                score = _name_match_score(name_lc, words)
                if score is None:
                    continue
            else:
                score = 0  # 空 query（纯类型搜索）：所有名字参与过滤
            if stat.S_ISDIR(mode):
                sz = None
            elif stat.S_ISREG(mode):
                sz = size
            else:
                continue  # symlink 等不参与匹配
            if exts:
                dot = name_lc.rfind(".")
                if dot < 0 or name_lc[dot + 1:] not in exts:
                    continue
            if after is not None and mtime < after:
                continue
            if before is not None and mtime >= before:
                continue
            if min_size is not None or max_size is not None:
                # 大小过滤时目录不参与（与 extensions 过滤目录语义一致：直接排除）
                if sz is None:
                    continue
                if min_size is not None and sz < min_size:
                    continue
                if max_size is not None and sz > max_size:
                    continue
            matches[os.path.normcase(os.path.normpath(entry_path))] = {
                "name": name,
                "path": entry_path,
                "size": sz,
                "size_human": None if sz is None else human_size(sz),
                "modified": _fmt_mtime(mtime),
                "mtime": mtime,   # 排序用数值（内部字段，输出前剔除）
                "score": score,   # 相关度评分（内部字段，输出前剔除）
                "is_dir": sz is None,
            }
            if len(matches) >= match_cap:
                state["hit_limit"] = True
                return


def _parse_date_filter(name, value):
    """解析时间过滤为本地时区时间戳。支持两种形式：
    - YYYY-MM-DD（after 含当日 00:00，before 不含当日）
    - 相对天数 'Nd'（如 7d = 最近 7 天含今天）——模型不必知道当前日期，
      用户说"上周/最近几天"直接转述为 Nd，日期推算由本机完成（实测模型
      推算日期会把年份算错，这是设计上必须避免的）。
    返回 (ts, None) 或 (None, error_message)。"""
    s = str(value).strip()
    m = re.match(r"^(\d{1,4})\s*d$", s, re.IGNORECASE)
    if m:
        days = int(m.group(1))
        today = datetime.date.today()
        if name == "modified_after":
            # 最近 N 天含今天：N-1 天前的 00:00
            return datetime.datetime.combine(
                today - datetime.timedelta(days=days - 1), datetime.time.min).timestamp(), None
        # before：早于 N 天前的 00:00
        return datetime.datetime.combine(
            today - datetime.timedelta(days=days), datetime.time.min).timestamp(), None
    try:
        d = datetime.date.fromisoformat(s)
    except ValueError:
        return None, "%s 必须是 YYYY-MM-DD 或相对天数（如 7d=最近 7 天）: %r" % (name, value)
    if name == "modified_after":
        return datetime.datetime.combine(d, datetime.time.min).timestamp(), None
    return datetime.datetime.combine(d + datetime.timedelta(days=1), datetime.time.min).timestamp(), None


def _parse_size_mb(name, value):
    """解析 MB 数值过滤为字节数；非法（非数字/NaN/Inf/负数/bool）→ (None, error)。"""
    if isinstance(value, bool):
        return None, "%s 不能是布尔值：%r" % (name, value)
    try:
        v = float(value)
    except (TypeError, ValueError):
        return None, "%s 必须是数字（MB）：%r" % (name, value)
    if not math.isfinite(v):
        return None, "%s 必须是有限数字：%r" % (name, value)
    if v < 0:
        return None, "%s 不能为负数：%r" % (name, value)
    return int(v * 1024 * 1024), None


def file_find(query="", limit=20, dir=None, extensions=None,
              modified_after=None, modified_before=None,
              min_size_mb=None, max_size_mb=None,
              sort_by="relevance", order="desc", exclude_dirs=None):
    """按概率序搜索文件/目录名（多词 AND + 相关度评分），满 limit 或超预算即停。
    extensions: 扩展名过滤（目录不参与）；modified_after/modified_before: YYYY-MM-DD；
    min_size_mb/max_size_mb: 大小过滤（MB，只作用于文件）；
    sort_by: relevance(默认，全词>前缀>子串，同分按修改时间)/mtime/size/name；
    order: desc(默认)/asc；exclude_dirs: 额外排除的同名目录。"""
    query = (query or "").strip()
    try:
        limit = int(limit)
    except (TypeError, ValueError):
        limit = 20
    limit = max(1, min(limit, FIND_MAX_LIMIT))

    sort_by = (sort_by or "relevance").strip().lower()
    if sort_by == "modified":
        sort_by = "mtime"  # 模型直觉别名（"按修改时间"）；评估实测模型首猜 modified
    if sort_by not in ("relevance", "mtime", "size", "name"):
        return {"error": "sort_by 仅支持 relevance/mtime/size/name（modified 是 mtime 的别名）"}
    order = (order or "desc").strip().lower()
    if order not in ("desc", "asc"):
        return {"error": "order 仅支持 desc/asc"}
    reverse = order == "desc"

    exts = None
    if extensions:
        if isinstance(extensions, str):
            extensions = [e for e in extensions.split(",") if e.strip()]
        exts = {str(e).strip().lstrip(".").lower() for e in (extensions or []) if str(e).strip()}
        if not exts:
            return {"error": "extensions 不能全为空"}
    filters = {"exts": exts} if exts else None

    after_ts = before_ts = None
    if modified_after:
        after_ts, err = _parse_date_filter("modified_after", modified_after)
        if err:
            return {"error": err}
    if modified_before:
        before_ts, err = _parse_date_filter("modified_before", modified_before)
        if err:
            return {"error": err}
    if after_ts is not None or before_ts is not None:
        if filters is None:
            filters = {}
        filters["after"] = after_ts
        filters["before"] = before_ts

    min_size = max_size = None
    if min_size_mb not in (None, ""):
        min_size, err = _parse_size_mb("min_size_mb", min_size_mb)
        if err:
            return {"error": err}
    if max_size_mb not in (None, ""):
        max_size, err = _parse_size_mb("max_size_mb", max_size_mb)
        if err:
            return {"error": err}
    if min_size is not None and max_size is not None and min_size > max_size:
        return {"error": "min_size_mb 不能大于 max_size_mb"}
    if min_size is not None or max_size is not None:
        if filters is None:
            filters = {}
        filters["min_size"] = min_size
        filters["max_size"] = max_size

    prune_extra = None
    if exclude_dirs:
        if isinstance(exclude_dirs, str):
            exclude_dirs = [exclude_dirs]
        prune_extra = {str(d).strip().lower() for d in exclude_dirs if str(d).strip()}

    # query 可选：空 query = 纯类型搜索（"找所有安装包/图片"类场景），但必须带过滤
    # 条件防返回全盘。多词为 AND（文件名需同时包含所有词）；要找一类文件用 extensions，
    # 不要把多个候选词一起传（"install setup 安装" 会因 AND 全命中而必 miss）。
    has_filter = bool(exts or after_ts is not None or before_ts is not None
                      or min_size is not None or max_size is not None)
    if not query:
        if not has_filter:
            return {"error": "query 为空时需提供过滤条件（extensions/min_size_mb/max_size_mb/"
                              "modified_after/modified_before），避免返回全盘文件"}
        words = []
    else:
        words = _split_query(query)
        if not words:
            # 全标点/单字符等被分词掏空：回退整串子串（与历史行为一致，不报错）
            words = [query.lower()]

    home = os.path.expanduser("~")
    if dir:
        root = os.path.abspath(os.path.expanduser(str(dir)))
        if not os.path.isdir(root):
            return {"error": "dir 不是已存在的目录: %s" % dir}
        priority_roots, fallback_root = [root], None
        coverage = "定向搜索 %s" % root
    else:
        priority_roots = [
            os.path.join(home, d) for d in PRIORITY_DIR_NAMES if os.path.isdir(os.path.join(home, d))
        ]
        fallback_root = home if os.path.isdir(home) else None
        coverage = ("按概率序搜索 " + "/".join(PRIORITY_DIR_NAMES) +
                    "（存在的），再对用户主目录整体兜底（含 AppData 下应用目录）")

    deadline = time.monotonic() + FIND_TIME_BUDGET_SEC
    matches = {}
    state = {"hit_limit": False, "timed_out": False}
    searched = []
    # 内部收集上限 = 3×limit：满 limit 即停会导致 total_hits 恒等于 count（截断信号失效），
    # 且 relevance 排序只在先收集到的 limit 个候选中做（非全局最优）
    collect_cap = min(limit * 3, FIND_MAX_LIMIT * 3)

    if priority_roots:
        _iter_search(words, priority_roots, [], deadline, matches, collect_cap, state,
                     filters=filters, visited=_seed_visited(priority_roots),
                     prune_extra=prune_extra)
        searched.extend(priority_roots)
    if fallback_root and not state["hit_limit"] and not state["timed_out"]:
        # visited 同时预种 priority roots：这些目录（或其 junction 别名）已在上面搜过，
        # fallback 遇到时直接剪掉，避免经 junction 重复遍历一遍
        _iter_search(words, [fallback_root], priority_roots,
                     deadline, matches, collect_cap, state,
                     filters=filters, visited=_seed_visited([fallback_root] + priority_roots),
                     prune_extra=prune_extra)
        searched.append(fallback_root)

    results = list(matches.values())
    # 末级兜底 name（升序）先排：同分同 mtime 时结果确定性（稳定排序保持该序）
    results.sort(key=lambda r: r["name"].lower())
    if sort_by == "relevance":
        # 相关度优先；order=asc 时分数升序，同分仍按修改时间从新到旧
        if order == "desc":
            results.sort(key=lambda r: (-r["score"], -r["mtime"]))
        else:
            results.sort(key=lambda r: (r["score"], -r["mtime"]))
    elif sort_by == "mtime":
        results.sort(key=lambda r: r["mtime"], reverse=reverse)
    elif sort_by == "size":
        # 目录 size=None 恒排最后（不能靠 reverse 翻转，需两分支固定目录末尾）
        if order == "desc":
            results.sort(key=lambda r: (r["size"] is None, -(r["size"] or 0)))
        else:
            results.sort(key=lambda r: (r["size"] is None, r["size"] or 0))
    else:  # name：大小写不敏感
        results.sort(key=lambda r: r["name"].lower(), reverse=reverse)
    total_hits = len(results)  # 压缩前收集到的全部命中数（截断信号 + 保险丝 note 用）
    results = results[:limit]
    for r in results:  # 剔除内部字段
        r.pop("mtime", None)
        r.pop("score", None)
    truncated = state["hit_limit"] or state["timed_out"] or total_hits > limit
    notes = [coverage]
    if not results and not dir:
        notes.append("默认范围仅主目录；目标可能在其他盘（如 D:\\），可用 dir 参数定向重搜（如 dir=\"D:\\\\\"）")
    if not words:
        notes.append("无关键词匹配（纯类型搜索，按过滤条件全量列出；时间预算内可能不全，可用 dir 定向）")
    elif len(words) > 1:
        notes.append("多词 AND（%s）：所有词都需命中" % "/".join(words))
    if filters:
        parts = []
        if exts:
            parts.append("扩展名限定 ." + "/.".join(sorted(exts)))
        if after_ts is not None:
            parts.append("修改时间不早于 %s" % modified_after)
        if before_ts is not None:
            parts.append("修改时间早于 %s" % modified_before)
        if min_size is not None:
            parts.append("大小 ≥ %s" % min_size_mb)
        if max_size is not None:
            parts.append("大小 ≤ %s" % max_size_mb)
        notes.append("已按 " + "、".join(parts) + " 过滤")
    notes.append({
        "relevance": "按相关度排序（全词>前缀>子串），同分按修改时间",
        "mtime": "按修改时间排序",
        "size": "按大小排序（目录恒排最后）",
        "name": "按名称排序（大小写不敏感）",
    }[sort_by] + ("（升序）" if order == "asc" else ""))
    if state["timed_out"]:
        notes.append("已超过 %d 秒时间预算提前停止，结果可能不全" % int(FIND_TIME_BUDGET_SEC))
    if state["hit_limit"]:
        notes.append("命中数超过收集上限，排序基于已收集结果，可换更精确关键词")
    notes.append("已剪枝 .ssh/.gnupg/node_modules/.git 等隐私与高 churn 目录")
    out = {
        "type": "file_find",
        "query": query,
        "count": len(results),
        # 收集到的全部命中数：count 是返回条数（受 limit 截断），total_hits > count
        # 说明结果不全——模型据此决定加 limit 或 dir 定向，避免分段穷举验证"找全"
        "total_hits": total_hits,
        "results": results,
        "searched_dirs": searched,
        "truncated": truncated,
        "note": "；".join(notes),
    }
    # 输出保险丝：基于最终 dict 测量（含 note/searched_dirs，避免低估）。
    # 先去 size 字节字段（保留 size_human），仍超则二分缩减条数。
    trimmed = False
    if _json_len(out) > OUTPUT_BUDGET_CHARS:
        trimmed = True
        for r in results:
            r.pop("size", None)
        while len(results) > 1 and _json_len(out) > OUTPUT_BUDGET_CHARS:
            results = results[:max(1, len(results) // 2)]
            out["results"] = results
            out["count"] = len(results)
        out["note"] = "；".join(notes + ["共命中 %d 条，输出超过底座上限，已缩减为前 %d 条"
                                         "（可用 dir/关键词收窄）" % (total_hits, len(results))])
        out["truncated"] = True  # 与超时/限流同语义：结果可能不全
    return out


# ── disk_scan ────────────────────────────────────────────────────────────

def _dir_stats(root, deadline, max_depth=SCAN_MAX_DEPTH, visited=None):
    """递归求和 (size, file_count, estimated, denied_root)。跳过 symlink 与不可读项。
    visited: reparse 目录真实路径集合，防 junction 环与重复求和（见 _reparse_target_realpath）。"""
    total, count, estimated = 0, 0, False
    seen = visited if visited is not None else _seed_visited([root])
    stack = [(root, 0)]
    while stack:
        if time.monotonic() > deadline:
            estimated = True
            break
        path, depth = stack.pop()
        if depth > max_depth:
            estimated = True
            continue
        entries = _cached_scandir(path)
        if entries is None:
            if path == root:
                return 0, 0, estimated, True
            continue
        for name, mode, size, mtime, rtag in entries:
            if stat.S_ISDIR(mode):
                rp = _reparse_target_realpath(os.path.join(path, name), mode, rtag)
                if rp is not None:
                    if rp in seen:
                        continue
                    seen.add(rp)
                stack.append((os.path.join(path, name), depth + 1))
            elif stat.S_ISREG(mode):
                total += size
                count += 1
    return total, count, estimated, False


def _scan_group(group, deadline):
    """扫描一组路径：总量 = 各根的直接子项求和（顺带产出 Top 子项）。"""
    total, count = 0, 0
    estimated, denied = False, False
    existing = [p for p in group["paths"] if p and os.path.isdir(p)]
    if not existing:
        return None  # 组路径全部不存在（未安装该应用）→ 省略该组
    children = []
    for root in existing:
        try:
            with os.scandir(root) as it:
                entries = list(it)
        except PermissionError:
            denied = True
            continue
        except OSError:
            continue
        for e in entries:
            if time.monotonic() > deadline:
                estimated = True
                break
            try:
                if e.is_symlink():
                    continue
                if e.is_dir(follow_symlinks=False):
                    size, cnt, est, den = _dir_stats(e.path, deadline)
                    estimated = estimated or est
                    denied = denied or den
                    is_dir = True
                elif e.is_file(follow_symlinks=False):
                    size, cnt, is_dir = e.stat(follow_symlinks=False).st_size, 1, False
                else:
                    continue
            except (PermissionError, OSError):
                continue
            total += size
            count += cnt
            children.append({
                "name": e.name,
                "path": e.path,
                "size_bytes": size,
                "size_human": human_size(size),
                "is_dir": is_dir,
            })
    children.sort(key=lambda c: c["size_bytes"], reverse=True)
    if denied and total == 0 and not children:
        status = "denied"
    elif estimated:
        status = "estimated"
    else:
        status = "ok"
    # 输出瘦身（底座工具结果 12,000 字符硬上限，超出会被压缩丢尾部）：
    # 不带 paths/size_bytes，top_children 只留前 3（删除目标从这里取全路径）。
    return {
        "key": group["key"],
        "name": group["name"],
        "status": status,
        "size_bytes": total,
        "size_human": human_size(total),
        "file_count": count,
        "top_children": [
            {"name": c["name"], "path": c["path"], "size_human": c["size_human"],
             "is_dir": c["is_dir"]}
            for c in children[:3]
        ],
        "risk": group["risk"],
        "note": group["note"],
    }


def _scan_groups():
    """静态规则表：Windows 常见文件积聚地（非 Windows 仅保留少量适用组）。"""
    profile = os.environ.get("USERPROFILE") or os.path.expanduser("~")
    if PLATFORM != "win32":
        home = os.path.expanduser("~")
        groups = [
            {"key": "temp", "name": "临时文件", "paths": [tempfile.gettempdir()],
             "risk": "green", "note": "系统与应用临时文件，删除安全"},
            {"key": "downloads", "name": "下载", "paths": [os.path.join(home, "Downloads")],
             "risk": "yellow", "note": "下载的文件，含用户数据，需逐项判断"},
            {"key": "dev_caches", "name": "开发缓存",
             "paths": [os.path.join(home, d) for d in
                       (".cache", ".npm", ".gradle", ".m2", ".cargo")],
             "risk": "green", "note": "包管理器与构建工具缓存，删除后自动重建"},
        ]
        if PLATFORM == "darwin":
            # 注：不单列浏览器缓存组——Library/Caches 根已覆盖（避免重叠双计）
            groups += [
                {"key": "library_caches", "name": "应用缓存 (Library/Caches)",
                 "paths": [os.path.join(home, "Library", "Caches")],
                 "risk": "green", "note": "macOS 应用缓存（含 Safari/Chrome 等浏览器缓存），删除后应用会重建，个别应用可能重新下载"},
                {"key": "library_appsupport", "name": "应用数据 (Library/Application Support)",
                 "paths": [os.path.join(home, "Library", "Application Support")],
                 "risk": "yellow", "note": "应用配置与数据，删除会影响应用设置，需逐项判断"},
                {"key": "xcode_derived", "name": "Xcode 构建缓存",
                 "paths": [os.path.join(home, "Library", "Developer", "Xcode", "DerivedData")],
                 "risk": "green", "note": "Xcode 编译产物，删除后下次构建自动重建"},
                {"key": "logs", "name": "系统与应用日志", "paths": [os.path.join(home, "Library", "Logs")],
                 "risk": "green", "note": "日志文件，删除安全"},
                {"key": "trash", "name": "废纸篓", "paths": [os.path.join(home, ".Trash")],
                 "risk": "yellow", "note": "废纸篓中的文件（含手动删除的），清空=物理删除不可恢复，需用户确认"},
            ]
        else:  # linux
            # 注：不单列浏览器缓存组——dev_caches 的 ~/.cache 根已覆盖
            groups += [
                {"key": "trash", "name": "回收站 (XDG Trash)",
                 "paths": [os.path.join(home, ".local", "share", "Trash")],
                 "risk": "yellow", "note": "回收站中的文件（含手动删除的），清空=物理删除不可恢复，需用户确认"},
            ]
        return groups
    local = os.environ.get("LOCALAPPDATA") or os.path.join(profile, "AppData", "Local")
    roaming = os.environ.get("APPDATA") or os.path.join(profile, "AppData", "Roaming")
    temp = os.environ.get("TEMP") or os.path.join(local, "Temp")
    pf = os.environ.get("ProgramFiles") or r"C:\Program Files"
    pfx = os.environ.get("ProgramFiles(x86)") or r"C:\Program Files (x86)"
    return [
        {"key": "temp", "name": "用户临时文件", "paths": [temp],
         "risk": "green", "note": "系统与应用产生的临时文件（%TEMP%），删除安全"},
        {"key": "crash_dumps", "name": "崩溃转储",
         "paths": [os.path.join(local, "CrashDumps")],
         "risk": "green", "note": "程序崩溃留下的诊断转储，删除安全"},
        {"key": "downloads", "name": "下载",
         "paths": [os.path.join(profile, "Downloads")],
         "risk": "yellow", "note": "浏览器/通讯软件下载的文件，含安装包与用户资料，需逐项判断"},
        {"key": "appdata_local", "name": "AppData 本地数据", "paths": [local],
         "risk": "yellow", "note": "应用本地数据与缓存大杂烩，子项需逐个判断"},
        {"key": "appdata_roaming", "name": "AppData 漫游数据", "paths": [roaming],
         "risk": "yellow", "note": "应用漫游配置与数据，删除会影响应用设置"},
        {"key": "wechat", "name": "微信文件",
         "paths": [os.path.join(profile, "Documents", "WeChat Files"),
                   os.path.join(profile, "Documents", "xwechat_files")],
         "risk": "yellow", "note": "微信聊天收发文件（🟡）；其中 FileStorage\\Cache、Applet 子目录是纯缓存（🟢）"},
        {"key": "dingtalk", "name": "钉钉数据",
         "paths": [os.path.join(roaming, "DingTalk")],
         "risk": "yellow", "note": "钉钉应用数据，含聊天附件，需逐项判断"},
        {"key": "feishu", "name": "飞书数据",
         "paths": [os.path.join(roaming, "bytedance")],
         "risk": "yellow", "note": "飞书/Lark 应用数据，含聊天附件，需逐项判断"},
        {"key": "chrome", "name": "Chrome 用户数据",
         "paths": [os.path.join(local, "Google", "Chrome", "User Data")],
         "risk": "yellow", "note": "整体含书签/登录态（🟡）；各 Profile 下 Cache、Code Cache 是纯缓存（🟢）"},
        {"key": "edge", "name": "Edge 用户数据",
         "paths": [os.path.join(local, "Microsoft", "Edge", "User Data")],
         "risk": "yellow", "note": "整体含书签/登录态（🟡）；各 Profile 下 Cache、Code Cache 是纯缓存（🟢）"},
        {"key": "dev_caches", "name": "开发缓存",
         "paths": [os.path.join(profile, ".cache"), os.path.join(profile, ".npm"),
                   os.path.join(profile, ".gradle"), os.path.join(profile, ".m2"),
                   os.path.join(profile, ".nuget", "packages"),
                   os.path.join(profile, ".cargo"),
                   os.path.join(local, "pip", "Cache"), os.path.join(local, "Yarn"),
                   os.path.join(local, "uv"), os.path.join(local, "ms-playwright")],
         "risk": "green", "note": "npm/gradle/pip 等包管理器与构建工具缓存，删除后自动重建"},
        {"key": "steam", "name": "Steam 游戏库",
         "paths": [os.path.join(pfx, "Steam")],
         "risk": "red", "note": "游戏本体，删游戏请走 Steam 客户端的卸载"},
        {"key": "program_files", "name": "Program Files", "paths": [pf],
         "risk": "red", "note": "已安装程序本体（64 位），请通过系统设置卸载"},
        {"key": "program_files_x86", "name": "Program Files (x86)", "paths": [pfx],
         "risk": "red", "note": "已安装程序本体（32 位），请通过系统设置卸载"},
    ]


def _list_drives():
    """枚举 Windows 所有逻辑盘符（'C:\\', 'D:\\', ...）。非 Windows → []。"""
    if PLATFORM != "win32":
        return []
    import ctypes
    mask = ctypes.windll.kernel32.GetLogicalDrives()
    drives = []
    for i in range(26):
        if mask & (1 << i):
            drives.append("%s:\\" % chr(ord("A") + i))
    return drives


def _scan_other_drives(overall_deadline):
    """概览附带的非系统盘信息：每盘容量/剩余 + 根目录直接子项 Top3。
    复用 _scan_group 拿 size/top_children/status 四件套；与主分组共享总预算，
    预算耗尽标 skipped。其他盘一律归 🟡（含用户数据，只给画像不主动删）。"""
    system = (os.environ.get("SystemDrive", "C:") + "\\") if PLATFORM == "win32" else None
    out = []
    for drive in _list_drives():
        if system and os.path.normcase(drive) == os.path.normcase(system):
            continue
        try:
            usage = shutil.disk_usage(drive)
        except OSError:
            out.append({"drive": drive, "error": "无法读取（光驱无盘/网络盘离线等）"})
            continue
        info = {
            "drive": drive,
            "total": human_size(usage.total),
            "used": human_size(usage.used),
            "free": human_size(usage.free),
            "free_percent": round(usage.free * 100.0 / usage.total, 1) if usage.total else None,
        }
        if time.monotonic() > overall_deadline:
            info["status"] = "skipped"
            info["note"] = "总时间预算耗尽，未扫描根目录"
            out.append(info)
            continue
        deadline = min(time.monotonic() + SCAN_GROUP_BUDGET_SEC, overall_deadline)
        scanned = _scan_group({
            "key": "drive_" + drive[0].lower(),
            "name": "%s 盘根目录" % drive[0],
            "paths": [drive],
            "risk": "yellow",
            "note": "非系统盘根目录直接子项（只读），大目录可在对话里对 path 下钻",
        }, deadline)
        if scanned:
            info["status"] = scanned["status"]
            info["size_human"] = scanned["size_human"]
            info["top_children"] = scanned["top_children"]
        else:
            info["status"] = "empty"
        out.append(info)
    return out


def _large_files(home, deadline):
    """主目录下 >500MB 的大文件（限时限深，剪枝同 file_find，防 junction 环）。"""
    found = []
    truncated = False
    seen = _seed_visited([home])
    stack = [(home, 0)]
    while stack:
        if time.monotonic() > deadline:
            truncated = True
            break
        path, depth = stack.pop()
        if depth > 8:
            truncated = True
            continue
        entries = _cached_scandir(path)
        if entries is None:
            continue
        for name, mode, size, mtime, rtag in entries:
            entry_path = os.path.join(path, name)
            if stat.S_ISDIR(mode):
                if name.lower() in PRUNE_DIR_NAMES:
                    continue
                rp = _reparse_target_realpath(entry_path, mode, rtag)
                if rp is not None:
                    if rp in seen:
                        continue
                    seen.add(rp)
                stack.append((entry_path, depth + 1))
            elif stat.S_ISREG(mode) and size >= LARGE_FILE_MIN_BYTES:
                found.append({
                    "name": name,
                    "path": entry_path,
                    "size_bytes": size,
                    "size_human": human_size(size),
                    "modified": _fmt_mtime(mtime),
                })
    found.sort(key=lambda f: f["size_bytes"], reverse=True)
    return found[:LARGE_FILE_LIMIT], truncated


def disk_scan(path=None, refresh=False):
    """双模式：无 path = 概览（分组总量）；有 path = 下钻（该目录直接子项按大小排序）。
    全部只读。分层调用使单次输出始终远小于工具结果压缩上限（12,000 字符）。"""
    if path:
        return _drill_down(path)

    started = time.time()
    overall_deadline = started + SCAN_TOTAL_BUDGET_SEC
    groups_out = []
    for group in _scan_groups():
        if time.monotonic() > overall_deadline:
            groups_out.append({
                "key": group["key"], "name": group["name"],
                "status": "skipped", "size_bytes": 0, "size_human": "0 B",
                "file_count": 0, "top_children": [], "risk": group["risk"],
                "note": group["note"] + "（总时间预算耗尽，未扫描）",
            })
            continue
        deadline = min(time.monotonic() + SCAN_GROUP_BUDGET_SEC, overall_deadline)
        scanned = _scan_group(group, deadline)
        if scanned is not None:
            groups_out.append(scanned)

    home = os.path.expanduser("~")
    large, large_truncated = _large_files(home, overall_deadline)
    other_drives = _scan_other_drives(overall_deadline)

    drive = (os.environ.get("SystemDrive", "C:") + "\\") if PLATFORM == "win32" else "/"
    disk = {}
    try:
        usage = shutil.disk_usage(drive)
        disk = {
            "drive": drive,
            "total": human_size(usage.total),
            "used": human_size(usage.used),
            "free": human_size(usage.free),
            "free_percent": round(usage.free * 100.0 / usage.total, 1) if usage.total else None,
        }
    except OSError as e:
        disk = {"drive": drive, "error": str(e)}

    overview = {
        "type": "disk_scan_overview",
        "generated_at": _fmt_mtime(started),
        "scan_seconds": round(time.time() - started, 1),
        "disk": disk,
        "groups": groups_out,
        "drives": other_drives,
        "large_files": large,
        "large_files_note": ("用户主目录下 >500MB 的大文件（最多 %d 条）" % LARGE_FILE_LIMIT) +
                            ("；扫描超时已截断，结果可能不全" if large_truncated else ""),
        "risk_legend": RISK_LEGEND,
        "note": ("只读扫描，未修改任何文件。要查看某组/某个大文件夹里具体是什么，"
                 "用 path 参数对该目录再调本工具逐层下钻；status=estimated 为限深/限时估算，"
                 "denied 为无权限。drives 段展示非系统盘容量与根目录大子项（一律 🟡，"
                 "大目录可用 path 下钻）。🟢 可放心清，🟡 需用户逐项判断，🔴 不提供删除。"),
    }
    # 输出保险丝：多盘/多组机器可能超底座上限。
    # 折叠顺序：drives（top_children 深路径是字符大头）→ groups → large_files 最后砍
    # （大文件清单最可行动，优先保留）。
    folded = False
    while len(other_drives) > 1 and _json_len(overview) > OUTPUT_BUDGET_CHARS:
        other_drives = other_drives[:max(1, len(other_drives) // 2)]
        overview["drives"] = other_drives
        folded = True
    while len(groups_out) > 1 and _json_len(overview) > OUTPUT_BUDGET_CHARS:
        groups_out = groups_out[:max(1, len(groups_out) // 2)]
        overview["groups"] = groups_out
        folded = True
    while len(large) > 1 and _json_len(overview) > OUTPUT_BUDGET_CHARS:
        large = large[:max(1, len(large) // 2)]
        overview["large_files"] = large
        folded = True
    if folded:
        overview["note"] += " 输出超限已折叠部分分组/盘符/大文件（large_files=%d、drives=%d、groups=%d）。" % (
            len(large), len(other_drives), len(groups_out))
    return overview


def _drill_down(path):
    """下钻：列出 path 的直接子项（目录递归求和、文件取大小），按大小降序 Top 20。
    每层一次调用，输出有界；预算耗尽标 estimated，剩余项标剩余个数。"""
    started = time.time()
    deadline = started + SCAN_GROUP_BUDGET_SEC
    if not isinstance(path, str) or not path.strip():
        return {"type": "error", "error": "path 不能为空"}
    p = path.strip()
    if not os.path.isabs(p):
        return {"type": "error", "error": "path 必须是绝对路径"}
    if not os.path.isdir(p):
        return {"type": "error", "error": "目录不存在或不是目录: %s" % p}

    children, estimated, hidden = [], False, 0
    try:
        entries = list(os.scandir(p))
    except (PermissionError, OSError) as e:
        return {"type": "error", "error": "无法读取目录: %s" % e}

    for e in entries:
        if time.monotonic() > deadline:
            estimated = True
            hidden += 1
            continue
        try:
            if e.is_symlink():
                continue
            if e.is_dir(follow_symlinks=False):
                size, _, est, _ = _dir_stats(e.path, deadline)
                estimated = estimated or est
                is_dir, est_flag = True, est
            elif e.is_file(follow_symlinks=False):
                size, is_dir = e.stat(follow_symlinks=False).st_size, False
                est_flag = False
            else:
                continue
        except (PermissionError, OSError):
            continue
        children.append({"name": e.name, "path": e.path, "size_bytes": size,
                         "size_human": human_size(size), "is_dir": is_dir,
                         "size_estimated": est_flag})

    children.sort(key=lambda c: c["size_bytes"], reverse=True)
    shown = children[:20]
    hidden += len(children) - len(shown)
    for c in shown:
        c.pop("size_bytes", None)
    note = "只读统计。可对某个大子目录继续用 path 下钻。"
    # 输出保险丝：深路径多时结果可能超底座上限，二分缩减条数（裁掉的计入 hidden）
    trimmed = 0
    while len(shown) > 1 and _json_len({"children": shown}) > OUTPUT_BUDGET_CHARS:
        shown = shown[:max(1, len(shown) // 2)]
        trimmed += 1
    hidden += trimmed
    if estimated:
        # 关键语义：estimated 只影响"子目录大小数值"（递归求和超时），
        # 直接子项清单本身完整无遗漏——避免模型误以为"清单可能不全"
        note = "子项清单完整；部分目录的大小为估算值（size_estimated=true 的项，递归求和超时）；" + note
    if hidden:
        note += " 另有 %d 个小项未显示。" % hidden
    if trimmed:
        note += " 输出超限，仅显示前 %d 条（可对子项逐个下钻）。" % len(shown)
    return {
        "type": "disk_scan_drill",
        "path": p,
        "scan_seconds": round(time.time() - started, 1),
        "children": shown,
        "estimated": estimated,
        "note": note,
    }


# ── file_trash ───────────────────────────────────────────────────────────

def _norm(p):
    return os.path.normcase(os.path.normpath(p))


def _is_under(path, root):
    """path == root 或 path 在 root 之下（Windows 大小写不敏感）。"""
    p, r = _norm(path), _norm(root)
    return p == r or p.startswith(r.rstrip(os.sep) + os.sep)


def _is_drive_root(p):
    n = os.path.normpath(p)
    if re.match(r"^[A-Za-z]:\\?$", n):
        return True
    if n.startswith("\\\\"):  # UNC 根 \\server\share
        return len([x for x in n.split("\\") if x]) <= 2
    return False


def _protected_roots():
    """硬拒绝区域（等于或在其之下都拒绝）。"""
    roots = []
    for env in ("SystemRoot", "windir"):
        v = os.environ.get(env)
        if v:
            roots.append(v)
    for env in ("ProgramFiles", "ProgramFiles(x86)", "ProgramW6432",
                "ProgramData", "CommonProgramFiles", "CommonProgramFiles(x86)"):
        v = os.environ.get(env)
        if v:
            roots.append(v)
    if PLATFORM == "win32":
        # 环境变量缺失时的兜底
        roots += [r"C:\Windows", r"C:\Program Files", r"C:\Program Files (x86)",
                  r"C:\ProgramData"]
        local = os.environ.get("LOCALAPPDATA") or os.path.join(
            os.path.expanduser("~"), "AppData", "Local")
        roots.append(os.path.join(local, "Programs", "Pinvou3"))  # pinvou3 安装目录
    roots.append(os.path.join(os.path.expanduser("~"), ".pinvou3"))
    return roots


def _validate_trash_target(raw, home, protected, allow_trash_root=False):
    """逐项白名单判定。返回 (abs_path 或 None, rejected_reason 或 None)。
    allow_trash_root=True（file_erase 用）：允许平台废纸篓根内的路径——
    file_erase 有独立的容器+日志准入；file_trash 仍拒绝（防废纸篓内嵌废纸篓）。"""
    if not isinstance(raw, str) or not raw.strip():
        return None, "空路径"
    if any(ch in raw for ch in "*?"):
        return None, "不接受通配符，请先展开为明确路径逐项核对"
    if not os.path.isabs(raw):
        return None, "必须是绝对路径"
    p = os.path.abspath(raw)
    if not os.path.lexists(p):
        return None, "路径不存在"
    if _is_drive_root(p):
        return None, "盘符根目录不可整体删除"
    for root in protected:
        if _is_under(p, root):
            return None, "系统/程序保护区域（%s），禁止删除" % root
    if _norm(p) == _norm(home):
        return None, "用户主目录本身不可删除"
    trash_root = _system_trash_root()
    if not allow_trash_root and trash_root and (
            _norm(p) == _norm(trash_root) or _is_under(p, trash_root)):
        return None, "平台废纸篓根目录不可作为删除目标（会形成废纸篓内嵌废纸篓）"
    return p, None


def _preview_size(path):
    """预览用大小：文件直接取；目录限时求和（超时标 estimated）。"""
    if os.path.isfile(path):
        try:
            return os.path.getsize(path), False
        except OSError:
            return 0, False
    deadline = time.monotonic() + TRASH_PREVIEW_SIZE_BUDGET_SEC
    size, _, estimated, _ = _dir_stats(path, deadline)
    return size, estimated


# Windows 回收站：ctypes 调 SHFileOperationW（纯 stdlib）。
# FOF_ALLOWUNDO 即“进回收站”；FOFX_RECYCLEONDELETE(0x80000) 塞不进
# SHFILEOPSTRUCTW 的 WORD fFlags（那是 IFileOperation 专属），故不可用、也不需要。
FO_DELETE = 0x0003
FOF_SILENT = 0x0004
FOF_NOCONFIRMATION = 0x0010
FOF_ALLOWUNDO = 0x0040
FOF_NOERRORUI = 0x0400


def _sh_file_operation_delete(path):
    import ctypes
    from ctypes import wintypes

    class SHFILEOPSTRUCTW(ctypes.Structure):
        _fields_ = [
            ("hwnd", wintypes.HWND),
            ("wFunc", wintypes.UINT),
            ("pFrom", wintypes.LPCWSTR),
            ("pTo", wintypes.LPCWSTR),
            ("fFlags", wintypes.USHORT),
            ("fAnyOperationsAborted", wintypes.BOOL),
            ("hNameMappings", wintypes.LPVOID),
            ("lpszProgressTitle", wintypes.LPCWSTR),
        ]

    # pFrom 必须是双 null 结尾的宽字符列表
    op = SHFILEOPSTRUCTW()
    op.hwnd = None
    op.wFunc = FO_DELETE
    op.pFrom = os.path.normpath(path) + "\0\0"
    op.pTo = None
    op.fFlags = FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI
    op.fAnyOperationsAborted = False
    op.hNameMappings = None
    op.lpszProgressTitle = None
    ret = ctypes.windll.shell32.SHFileOperationW(ctypes.byref(op))
    if ret != 0:
        raise OSError("SHFileOperationW 失败，返回 0x%X" % ret)
    if op.fAnyOperationsAborted:
        raise OSError("操作被中止")


def _recycle_quota(path):
    """path 所在盘的回收站配额字节数（Windows 默认 = 盘容量 5%）。

    自定义配额（注册表 MaxCapacity）未读取：按默认估算，结果偏保守——宁可
    多走 _pinvou_filemaster_trash 兜底，也不冒「Shell 静默物理删除」的风险（FOF_NOCONFIRMATION
    下超过配额的对象不提示、不进回收站，且 SHFileOperationW 仍返回成功，
    日志会误记 recycle-bin——pinkbin 事故的根因）。
    """
    if PLATFORM != "win32":
        return None
    drive = os.path.splitdrive(os.path.abspath(path))[0] + "\\"
    try:
        return int(shutil.disk_usage(drive).total * 0.05)
    except OSError:
        return None


def _prune_empty_trash_container(path):
    """恢复/清除后清理空的 _pinvou_filemaster_trash 容器链（不留空目录痕迹）。
    先向上定位 _pinvou_filemaster_trash 容器（找不到——如系统废纸篓 ~/.Trash、XDG Trash files/——
    直接返回，绝不误删）；找到后从内向外逐级删除空目录，止步于容器本身。
    绝不向上删 _pinvou_filemaster_trash 之外的用户目录。"""
    chain = []
    cur = os.path.dirname(path)
    while True:
        chain.append(cur)
        if os.path.basename(cur).lower() == TRASH_DIRNAME:
            break
        parent = os.path.dirname(cur)
        if parent == cur:
            return  # 路径不含 _pinvou_filemaster_trash 组件（系统废纸篓等）：不清理
        cur = parent
    for d in chain:  # 从最内层向上
        try:
            if not os.path.isdir(d) or os.listdir(d):
                return
            os.rmdir(d)
        except OSError:
            return


def _fallback_trash(path):
    """兜底：移入目标同级 `_pinvou_filemaster_trash/` 目录（时间戳前缀）。绝不物理删除。
    选名+移动在 _trash_move_lock 内（并发同名防静默覆盖）。"""
    parent = os.path.dirname(os.path.abspath(path))
    trash = os.path.join(parent, TRASH_DIRNAME)
    os.makedirs(trash, exist_ok=True)
    base = os.path.basename(path.rstrip("\\/")) or "item"
    with _trash_move_lock:
        dest = os.path.join(trash, "%s_%s" % (time.strftime("%Y%m%d-%H%M%S"), base))
        n = 1
        while os.path.exists(dest):
            dest = os.path.join(trash, "%s_%d_%s" % (time.strftime("%Y%m%d-%H%M%S"), n, base))
            n += 1
        shutil.move(path, dest)
    return dest


def _xdg_trash_dir():
    """XDG Trash 根目录（Linux）：$XDG_DATA_HOME/Trash 或 ~/.local/share/Trash。"""
    base = os.environ.get("XDG_DATA_HOME")
    if not base or not os.path.isabs(base):
        base = os.path.join(os.path.expanduser("~"), ".local", "share")
    return os.path.join(base, "Trash")


def _trash_unique_name(trash_dir, name):
    """在 trash_dir 内找不冲突的名字（"name"、"name 2"、"name 3"…，系统风格后缀）。"""
    dest = os.path.join(trash_dir, name)
    n = 2
    while os.path.lexists(dest):
        root, ext = os.path.splitext(name)
        dest = os.path.join(trash_dir, "%s %d%s" % (root, n, ext))
        n += 1
    return dest


def _move_to_system_trash(path):
    """macOS/Linux 移入系统废纸篓/XDG Trash，返回 dest（trash 内实际落点）。
    失败（跨卷/无权限等）返回 None，由调用方落 _pinvou_filemaster_trash 兜底。
    跨卷必须预检 st_dev：shutil.move 会吞 EXDEV 自动降级为跨卷复制（非原子且
    macOS 丢 xattr/资源分叉），系统废纸篓在外部卷另有 .Trashes/<uid> 机制。"""
    try:
        if PLATFORM == "darwin":
            trash_dir = os.path.join(os.path.expanduser("~"), ".Trash")
            if os.path.islink(trash_dir):
                return None  # .Trash 被替换成 symlink：拒绝，防删到链接目标
            os.makedirs(trash_dir, exist_ok=True)
            try:
                os.chmod(trash_dir, 0o700)  # 与 Finder 默认一致
            except OSError:
                pass
            if os.stat(path).st_dev != os.stat(trash_dir).st_dev:
                return None  # 跨卷：回退 _pinvou_filemaster_trash
            with _trash_move_lock:
                dest = _trash_unique_name(trash_dir, os.path.basename(path.rstrip("\\/")))
                shutil.move(path, dest)
            return dest
        if PLATFORM == "linux":
            trash_dir = _xdg_trash_dir()
            files_dir = os.path.join(trash_dir, "files")
            info_dir = os.path.join(trash_dir, "info")
            os.makedirs(files_dir, exist_ok=True)
            os.makedirs(info_dir, exist_ok=True)
            if os.stat(path).st_dev != os.stat(files_dir).st_dev:
                return None  # 跨卷：XDG 需挂载点 .Trash-<uid>，回退 _pinvou_filemaster_trash
            name = os.path.basename(path.rstrip("\\/"))
            with _trash_move_lock:
                dest = _trash_unique_name(files_dir, name)
                shutil.move(path, dest)
                # XDG Trash 规范：info/<name>.trashinfo——Path 需 RFC 2396
                # percent-encode（空格/#/%/换行必须编码）；DeletionDate 本地时区无偏移
                info_path = os.path.join(info_dir, os.path.basename(dest) + ".trashinfo")
                deletion = time.strftime("%Y-%m-%dT%H:%M:%S")
                with open(info_path, "w", encoding="utf-8") as f:
                    f.write("[Trash Info]\nPath=%s\nDeletionDate=%s\n" % (
                        urllib.parse.quote(os.path.abspath(path), safe="/"), deletion))
            return dest
    except OSError:
        return None
    return None


# 废纸篓选名+移动临界区：并发 worker 批量删同名文件时，防 POSIX rename 静默覆盖
_trash_move_lock = threading.Lock()


def _system_trash_root():
    """平台系统废纸篓内容根（darwin ~/.Trash；linux <XDG>/Trash/files；其余 None）。"""
    if PLATFORM == "darwin":
        return os.path.join(os.path.expanduser("~"), ".Trash")
    if PLATFORM == "linux":
        return os.path.join(_xdg_trash_dir(), "files")
    return None


def _move_to_recycle(path):
    """返回 (via, detail, dest)。Windows 优先系统回收站；macOS/Linux 优先系统
    废纸篓/XDG Trash（via=system-trash）；跨卷/失败兜底同级 _pinvou_filemaster_trash。
    dest：系统回收站方式时为 None；其余方式时为实际落点。"""
    if PLATFORM == "win32":
        try:
            _sh_file_operation_delete(path)
            return "recycle-bin", "已移入 Windows 回收站，可从回收站恢复", None
        except Exception:
            pass  # 落到兜底
    dest = _move_to_system_trash(path)
    if dest is not None:
        via = "system-trash"
        detail = ("已移入系统废纸篓（%s），可从废纸篓恢复" % dest) if PLATFORM == "darwin" \
            else ("已移入 XDG Trash（%s），可从回收站恢复" % dest)
        return via, detail, dest
    dest = _fallback_trash(path)
    return "fallback-trash-dir", "已移入同级 %s 目录: %s" % (TRASH_DIRNAME, dest), dest


def _execute_trash_item(item):
    """单项目执行（worker 用）：目标超过回收站配额（或大小估算不可靠）时，
    改走 _pinvou_filemaster_trash 兜底——Shell 在 FOF_NOCONFIRMATION 下对超配额对象会静默物理删除
    且返回成功（日志误记 recycle-bin、无法恢复）。_pinvou_filemaster_trash 兜底永远可 restore。"""
    size = item.get("size")
    estimated = item.get("size_estimated")
    quota = _recycle_quota(item["path"])
    if quota is not None and (estimated or (size is not None and size > quota)):
        dest = _fallback_trash(item["path"])
        detail = ("目标可能超过回收站配额（约 %s），直接删除会被 Shell 物理删除不可恢复；"
                  "已改用同级 %s 目录安全兜底（可恢复，但未释放磁盘空间）: %s"
                  % (human_size(quota), TRASH_DIRNAME, dest))
        return "fallback-trash-dir", detail, dest
    return _move_to_recycle(item["path"])


# ── 删除日志（落盘，恢复的唯一事实来源，不靠模型记忆）─────────────────────
# 异步删除 worker 与主线程（file_restore 标 restored）可能并发写日志：
# 读-改-写序列必须互斥，否则并发 append 会互相覆盖丢条目。

TRASH_LOG_MAX_ENTRIES = 200

_log_lock = threading.Lock()


def _trash_log_path():
    return os.path.join(os.path.expanduser("~"), ".pinvou3", "file-master", "trash-log.json")


def _read_trash_log():
    try:
        with open(_trash_log_path(), encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, list) else []
    except (OSError, ValueError):
        return []


def _write_trash_log(log):
    try:
        os.makedirs(os.path.dirname(_trash_log_path()), exist_ok=True)
        tmp = _trash_log_path() + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(log, f, ensure_ascii=False, indent=1)
        os.replace(tmp, _trash_log_path())
    except OSError:
        pass  # 日志写入失败不阻断主流程


def _append_trash_log(moved_items):
    """moved_items: [{path, via, detail, dest}]，追加 status=trashed 的记录。"""
    if not moved_items:
        return
    with _log_lock:
        log = _read_trash_log()
        now = _fmt_mtime(time.time())
        for it in moved_items:
            log.append({
                "time": now,
                "name": os.path.basename(it["path"].rstrip("\\/")),
                "original_path": it["path"],
                "via": it["via"],
                "dest": it.get("dest"),
                "status": "trashed",
            })
        _write_trash_log(log[-TRASH_LOG_MAX_ENTRIES:])


def _mark_trash_log_restored(original_path):
    """把最近一条匹配的 trashed 记录标记为 restored。"""
    with _log_lock:
        log = _read_trash_log()
        for e in reversed(log):
            if e.get("status") == "trashed" and _norm(e.get("original_path", "")) == _norm(original_path):
                e["status"] = "restored"
                e["restored_at"] = _fmt_mtime(time.time())
                _write_trash_log(log)
                return True
        return False


# ── 异步删除任务（内存状态，进程生命周期内）──────────────────────────────

_tasks_lock = threading.Lock()
_tasks = {}        # task_id -> task dict
_task_seq = [0]


def _next_task_id():
    with _tasks_lock:
        _task_seq[0] += 1
        return "%s-%d-%d" % (time.strftime("%Y%m%d%H%M%S"), os.getpid(), _task_seq[0])


def _prune_tasks():
    """惰性清理超 TTL 的已完成任务（running 永不清理）。"""
    now = time.time()
    with _tasks_lock:
        for tid in [t for t, v in _tasks.items()
                    if v["status"] == "done"
                    and now - v.get("finished_ts", now) > TRASH_TASK_TTL_SEC]:
            _tasks.pop(tid, None)


def _trash_worker(task_id):
    """后台线程体：逐项移入回收站，每成功一项立即写删除日志（进程被强杀时
    已完成项仍可恢复）。绝不写 stdout——stdio JSON-RPC 流只能主线程写。"""
    with _tasks_lock:
        task = _tasks.get(task_id)
    if task is None:
        return
    results = []
    for item in task["items"]:
        if not item["allowed"]:
            results.append({"path": item["path"], "status": "rejected",
                            "error": item["rejected_reason"]})
        else:
            try:
                via, detail, dest = _execute_trash_item(item)
                results.append({"path": item["path"], "status": "moved",
                                "via": via, "detail": detail})
                _append_trash_log([{"path": item["path"], "via": via, "dest": dest}])
            except Exception as e:
                results.append({"path": item["path"], "status": "error", "error": str(e)})
        with _tasks_lock:
            task["done_count"] += 1
            task["results"] = list(results)  # 进度快照，主线程轮询时可见
    with _tasks_lock:
        task["results"] = results
        task["summary"] = {
            "total": task["total_count"],
            "moved": sum(1 for r in results if r["status"] == "moved"),
            "failed": sum(1 for r in results if r["status"] == "error"),
            "rejected": sum(1 for r in results if r["status"] == "rejected"),
            "total_size_human": task["summary"].get("total_size_human"),
        }
        task["finished_at"] = _fmt_mtime(time.time())
        task["finished_ts"] = time.time()
        task["status"] = "done"


def file_trash(paths=None, confirm=False):
    """删除 = 移入回收站。confirm=false 只预览；confirm=true 逐项执行。"""
    if not isinstance(paths, list) or not paths:
        return {"error": "paths 必须是非空绝对路径数组"}
    if len(paths) > TRASH_MAX_PATHS:
        return {"error": "paths 最多 %d 条，请分批处理" % TRASH_MAX_PATHS}
    home = os.path.expanduser("~")
    protected = _protected_roots()

    items = []
    total_bytes = 0
    for raw in paths:
        p, reason = _validate_trash_target(raw, home, protected)
        if reason:
            items.append({"path": raw, "allowed": False, "rejected_reason": reason,
                          "warning": None, "size": None, "size_human": None})
            continue
        size, estimated = _preview_size(p)
        warning = None
        if not _is_under(p, home):
            warning = "不在用户主目录之下，请用户格外确认"
        quota = _recycle_quota(p)
        if quota is not None and (estimated or size > quota):
            warning = ("目标可能超过回收站配额（约 %s），执行时会改用 _pinvou_filemaster_trash 兜底"
                       "（可恢复，但未释放磁盘空间）" % human_size(quota))
        total_bytes += size
        items.append({
            "path": p,
            "allowed": True,
            "rejected_reason": None,
            "warning": warning,
            "size": size,
            "size_estimated": estimated,
            "size_human": ("约 " if estimated else "") + human_size(size),
        })

    allowed_count = sum(1 for i in items if i["allowed"])
    summary = {
        "total": len(items),
        "allowed": allowed_count,
        "rejected": len(items) - allowed_count,
        "total_size": total_bytes,
        "total_size_human": human_size(total_bytes),
    }

    if not confirm:
        return {
            "type": "file_trash_preview",
            "executed": False,
            "items": items,
            "summary": summary,
            "note": ("预览未执行，未改动任何文件。请把清单展示给用户；"
                     "用户明确确认后，用相同 paths + confirm=true 重调本工具执行。"),
        }

    # 异步提交：后台线程逐项执行，主线程立即返回（SHFileOperationW 同步阻塞
    # 大目录可能远超底座 30 秒 execute_timeout）。结果用 file_trash_status 轮询。
    task_id = _next_task_id()  # 锁外生成（_next_task_id 内部持锁，Lock 不可重入）
    with _tasks_lock:
        active = sum(1 for t in _tasks.values() if t["status"] == "running")
        if active >= TRASH_MAX_ACTIVE_TASKS:
            return {"error": "已有 %d 个删除任务在运行中，请先用 file_trash_status 查看/等待"
                    % active}
        task = {
            "task_id": task_id,
            "status": "running",
            "created_at": _fmt_mtime(time.time()),
            "items": items,
            "done_count": 0,
            "total_count": len(items),
            "results": [],
            "summary": summary,
        }
        _tasks[task_id] = task
    threading.Thread(target=_trash_worker, args=(task_id,), daemon=True).start()
    return {
        "type": "file_trash_submitted",
        "task_id": task_id,
        "items": items,
        "summary": summary,
        "note": ("删除已在后台执行（避免大目录阻塞）。用 file_trash_status(task_id=%s) "
                 "轮询直到 status=done，再向用户汇报逐项结果；文件移入回收站可恢复，"
                 "误删后可用 file_restore 一键还原。" % task_id),
    }


def file_trash_status(task_id=None, limit=10):
    """查询异步删除任务：有 task_id 查单个（running 含进度，done 含逐项结果）；
    无则列最近任务（先惰性清理过期任务）。"""
    _prune_tasks()
    try:
        limit = max(1, min(int(limit), 20))
    except (TypeError, ValueError):
        limit = TRASH_STATUS_MAX_LIST
    with _tasks_lock:
        if task_id:
            task = _tasks.get(task_id)
            if task is None:
                return {"type": "error", "error": "task 不存在或已过期: %s" % task_id}
            return {
                "type": "file_trash_status",
                "task_id": task_id,
                "status": task["status"],
                "kind": task.get("kind", "trash"),
                "created_at": task["created_at"],
                "done_count": task["done_count"],
                "total_count": task["total_count"],
                "results": task["results"] if task["status"] == "done" else None,
                "summary": task["summary"] if task["status"] == "done" else None,
            }
        tasks = sorted(_tasks.values(), key=lambda t: t["created_at"], reverse=True)[:limit]
        return {
            "type": "file_trash_tasks",
            "count": len(tasks),
            "tasks": [{"task_id": t["task_id"], "status": t["status"],
                       "done_count": t["done_count"], "total_count": t["total_count"],
                       "created_at": t["created_at"]} for t in tasks],
            "note": "列出最近的删除任务；用 file_trash_status(task_id=...) 查单个。",
        }


# ── file_restore ─────────────────────────────────────────────────────────

RESTORE_PS_TIMEOUT_SEC = 25


def _rb_root_for(drive):
    """盘符对应的回收站根目录（测试可注入假目录）。"""
    return drive + r"\$RECYCLE.BIN"


def _rb_locate(path):
    """在目标盘回收站按原始路径定位数据文件（$R*）。$I 元数据格式：
    header8 + 文件大小8 + FILETIME8 + 路径字节长度4 + UTF-16LE 路径。
    返回 $R 文件路径或 None。COM「还原」动词对某些条目不可用（NOVERB），
    这是 file_restore 的降级路径：绕过动词直接挪 $R 数据文件。"""
    if PLATFORM != "win32":
        return None
    drive = os.path.splitdrive(os.path.abspath(path))[0]
    if not drive:
        return None
    rb_root = _rb_root_for(drive)
    if not os.path.isdir(rb_root):
        return None
    target = _norm(path)
    for sid in os.listdir(rb_root):
        sdir = os.path.join(rb_root, sid)
        if not os.path.isdir(sdir):
            continue
        try:
            names = os.listdir(sdir)
        except OSError:
            continue
        for n in names:
            if not n.startswith("$I"):
                continue
            try:
                with open(os.path.join(sdir, n), "rb") as fh:
                    data = fh.read(1 << 20)
                if len(data) < 28:
                    continue
                # 路径从 28 偏移开始、UTF-16LE、null 字符结尾。
                # 不能用 split(b"\x00\x00")：字符高字节 00 与 null 首字节会形成
                # 伪边界丢掉末字符，必须按 2 字节对齐扫描 null 字符。
                raw = data[28:]
                end = len(raw)
                for i in range(0, len(raw) - 1, 2):
                    if raw[i] == 0 and raw[i + 1] == 0:
                        end = i
                        break
                orig = raw[:end].decode("utf-16-le", errors="replace")
                if not orig:
                    continue
            except (OSError, UnicodeDecodeError):
                continue
            if _norm(orig) == target:
                rfile = os.path.join(sdir, "$R" + n[2:])
                if os.path.lexists(rfile):
                    return rfile
    return None


def _rb_manual_restore(path):
    """降级还原：COM 动词不可用时，把回收站 $R 数据文件直接挪回原位置（同卷）。
    返回还原后的路径，或 None（未定位到/目标冲突/失败——调用方保持原错误）。"""
    rfile = _rb_locate(path)
    if not rfile:
        return None
    if os.path.exists(path):
        return None  # 目标已存在：不覆盖
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        shutil.move(rfile, path)
    except OSError:
        return None
    return path


def _ps_run(script):
    """调 Windows PowerShell 执行脚本，返回 (rc, stdout, stderr)。强制 UTF-8 输出。
    全程无窗口：server 由无控制台的 pythonw 拉起，console 子进程默认会被 Windows
    分配一个可见控制台窗口（每调一次闪一次），CREATE_NO_WINDOW + SW_HIDE 杜绝。"""
    cmd = [
        "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command",
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; " + script,
    ]
    kwargs = {}
    if PLATFORM == "win32":
        si = subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        si.wShowWindow = 0  # SW_HIDE
        kwargs["startupinfo"] = si
        kwargs["creationflags"] = subprocess.CREATE_NO_WINDOW
    r = subprocess.run(cmd, capture_output=True, timeout=RESTORE_PS_TIMEOUT_SEC, **kwargs)
    return (r.returncode,
            r.stdout.decode("utf-8", "replace"),
            r.stderr.decode("utf-8", "replace"))


# 按原始完整路径在回收站里定位并执行「还原」动词；打印 RESTORED:<原始路径> 或 NOTFOUND/NOVERB。
_RB_RESTORE_TEMPLATE = (
    "$bin=(New-Object -ComObject Shell.Application).NameSpace(0xA);"
    "$target='%s';$hit=$null;"
    "foreach($i in $bin.Items()){"
    "$orig=Join-Path $bin.GetDetailsOf($i,1) $i.Name;"
    "if($orig -ieq $target){$hit=$i;break}};"
    "if(-not $hit){'NOTFOUND'}else{"
    "$v=$hit.Verbs()|Where-Object{$_.Name -match '还原|Restore'}|Select-Object -First 1;"
    "if($v){$v.DoIt();'RESTORED:'+$target}else{'NOVERB'}}"
)


def _rb_unsupported():
    return {"type": "error",
            "error": "回收站还原目前仅支持 Windows（依赖 Shell.Application COM）"}


def file_restore(action="list", path=None, limit=20):
    """误删恢复。action=list 读删除日志（本工具删除的记录，落盘持久）；
    action=restore 按日志记录的 via 精确还原：recycle-bin 走 COM 还原动词，
    fallback-trash-dir 直接从 _pinvou_filemaster_trash 落点挪回原路径（确定性，不依赖回收站元数据）。"""
    action = (action or "list").strip().lower()

    if action == "list":
        try:
            limit = max(1, min(int(limit), 50))
        except (TypeError, ValueError):
            limit = 20
        entries = [e for e in _read_trash_log() if e.get("status") == "trashed"]
        entries.reverse()  # 最近删除在前
        items = [{
            "name": e.get("name"),
            "original_path": e.get("original_path"),
            "deleted": e.get("time"),
            "via": e.get("via"),
        } for e in entries[:limit]]
        # 输出保险丝：深路径条目多时结果可能超底座上限，二分缩减
        if items and _json_len({"items": items}) > OUTPUT_BUDGET_CHARS:
            while len(items) > 1 and _json_len({"items": items}) > OUTPUT_BUDGET_CHARS:
                items = items[:max(1, len(items) // 2)]
        note = ("删除日志中待恢复的记录（本工具删除的才会记录，最近在前，最多 %d 条）。"
                "要还原某条，用 action='restore' + path=<该条 original_path>。" % limit)
        if not items:
            note = ("删除日志为空或全部已恢复。注意：只有经 file_trash 删除的才会入日志；"
                    "其他方式删除的请用系统回收站界面恢复。")
        return {"type": "file_restore_list", "count": len(items), "items": items, "note": note}

    if action == "restore":
        if not isinstance(path, str) or not path.strip():
            return {"type": "error", "error": "restore 需要提供 path（日志条目的 original_path）"}
        target = path.strip()
        # 已被 file_erase 物理删除的记录：明确报错，避免误走回收站还原路径
        for e in reversed(_read_trash_log()):
            if e.get("status") == "erased" and _norm(e.get("original_path", "")) == _norm(target):
                return {"type": "error",
                        "error": "该文件已被 file_erase 物理删除，无法恢复: %s" % target}
        # 查日志定位删除方式
        entry = None
        for e in reversed(_read_trash_log()):
            if e.get("status") == "trashed" and _norm(e.get("original_path", "")) == _norm(target):
                entry = e
                break

        # 兜底删除方式：日志记录了精确落点，直接挪回（任何平台可用）
        if entry and entry.get("via") in ("fallback-trash-dir", "system-trash"):
            dest = entry.get("dest")
            if not dest or not os.path.exists(dest):
                return {"type": "error", "error": "_pinvou_filemaster_trash 中的备份已不存在（可能被手动清理）: %s" % dest}
            if os.path.exists(target):
                return {"type": "error", "error": "原位置已存在同名文件，为避免覆盖未还原: %s" % target}
            try:
                os.makedirs(os.path.dirname(target), exist_ok=True)
                shutil.move(dest, target)
            except OSError as e:
                return {"type": "error", "error": "还原失败: %s" % e}
            if entry.get("via") == "system-trash" and PLATFORM == "linux":
                # 清理对应 trashinfo，避免文件管理器幽灵条目
                # dest = <trash>/files/<name> → info = <trash>/info/<name>.trashinfo
                info_path = os.path.join(os.path.dirname(os.path.dirname(dest)),
                                         "info", os.path.basename(dest) + ".trashinfo")
                if os.path.exists(info_path):
                    try:
                        os.remove(info_path)
                    except OSError:
                        pass
            _mark_trash_log_restored(target)
            _prune_empty_trash_container(dest)  # 恢复后清理空的 _pinvou_filemaster_trash 容器（不留痕迹）
            return {"type": "file_restore_result", "status": "restored", "path": target,
                    "via": entry.get("via", "fallback-trash-dir"),
                    "note": "已从备份落点（%s）挪回原位置。" % ("_pinvou_filemaster_trash" if entry.get("via") == "fallback-trash-dir" else "废纸篓/Trash")}

        # 回收站方式（或无日志记录的历史删除）：走 COM 还原动词；
        # COM 失败（NOVERB/NOTFOUND/超时等）→ 降级按 $I 元数据定位 $R 数据文件挪回
        if PLATFORM != "win32":
            return _rb_unsupported()
        if "'" in target:
            return {"type": "error", "error": "path 含非法字符"}
        # PowerShell 单引号字符串不处理反斜杠转义（仅 '' 表转义单引号），路径原样嵌入即可。
        try:
            rc, out, err = _ps_run(_RB_RESTORE_TEMPLATE % target)
        except Exception as e:
            manual = _rb_manual_restore(target)
            if manual:
                _mark_trash_log_restored(target)
                return {"type": "file_restore_result", "status": "restored", "path": manual,
                        "via": "recycle-bin", "note": "已从回收站数据文件直接挪回原位置（COM 动词不可用，走降级路径）。"}
            return {"type": "error", "error": "还原失败: %s" % e}
        out = out.strip()
        if out.startswith("RESTORED:"):
            restored = out[len("RESTORED:"):].strip()
            ok = os.path.exists(restored)
            if ok:
                _mark_trash_log_restored(target)
            return {
                "type": "file_restore_result",
                "status": "restored" if ok else "verb_sent_but_missing",
                "path": restored,
                "via": "recycle-bin",
                "note": ("已从回收站还原到原位置。" if ok else
                         "还原命令已执行但目标位置未见到文件，请在回收站界面手动核对。"),
            }
        # COM 失败：尝试降级（$I 定位 + $R 挪回）
        manual = _rb_manual_restore(target)
        if manual:
            _mark_trash_log_restored(target)
            return {"type": "file_restore_result", "status": "restored", "path": manual,
                    "via": "recycle-bin", "note": "已从回收站数据文件直接挪回原位置（COM 还原动词不可用，走降级路径）。"}
        if out == "NOTFOUND":
            return {"type": "error",
                    "error": "回收站中未找到该路径（可能已被清空/还原，或 original_path 不一致）: %s" % target}
        if out == "NOVERB":
            return {"type": "error", "error": "该条目没有可用的「还原」动词: %s" % target}
        return {"type": "error", "error": "还原失败: %s" % (err.strip() or out[:200])}

    return {"type": "error", "error": "action 仅支持 list / restore"}


# ── 清空回收站 ───────────────────────────────────────────────────────────

SHERB_NOCONFIRMATION = 0x1
SHERB_NOPROGRESSUI = 0x2
SHERB_NOSOUND = 0x4
SHERB_FLAGS = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND


def _query_recycle_bin():
    """查询回收站占用（字节, 条目数）。三端：
    Windows 走 SHQueryRecycleBinW；macOS 枚举 ~/.Trash；Linux 枚举 XDG Trash files/。
    失败/不可用返回 (0, 0)。"""
    if PLATFORM == "win32":
        import ctypes
        from ctypes import wintypes

        class SHQUERYRBINFO(ctypes.Structure):
            _fields_ = [("cbSize", wintypes.DWORD),
                        ("i64Size", ctypes.c_longlong),
                        ("i64NumItems", ctypes.c_longlong)]

        info = SHQUERYRBINFO()
        info.cbSize = ctypes.sizeof(SHQUERYRBINFO)  # 必须先赋值，否则返回 E_INVALIDARG
        if ctypes.windll.shell32.SHQueryRecycleBinW(None, ctypes.byref(info)) != 0:
            return 0, 0
        return info.i64Size, info.i64NumItems
    trash_dir = _trash_contents_dir()
    if not trash_dir or not os.path.isdir(trash_dir):
        return 0, 0
    total, count = 0, 0
    try:
        with os.scandir(trash_dir) as it:
            for e in it:
                if e.name in (".DS_Store", "desktop.ini"):
                    continue
                try:
                    st = e.stat(follow_symlinks=False)
                except OSError:
                    continue
                total += st.st_size
                count += 1
    except OSError:
        pass
    return total, count


def _trash_contents_dir():
    """回收站内容目录：macOS ~/.Trash；Linux XDG Trash 的 files/；其他 → None。"""
    if PLATFORM == "darwin":
        return os.path.join(os.path.expanduser("~"), ".Trash")
    if PLATFORM == "linux":
        return os.path.join(_xdg_trash_dir(), "files")
    return None


def _empty_trash_dir():
    """macOS/Linux 清空：枚举内容目录直接子项，逐项 _erase_item（含 junction 预检
    与删除日志标记）。返回 (freed_bytes, count)。"""
    trash_dir = _trash_contents_dir()
    if not trash_dir or not os.path.isdir(trash_dir):
        return 0, 0
    freed, count = 0, 0
    with os.scandir(trash_dir) as it:
        entries = [e for e in it if e.name not in (".DS_Store", "desktop.ini")]
    for e in entries:
        try:
            st = e.stat(follow_symlinks=False)
            _erase_item(e.path, allow_symlink=True)  # 废纸篓内 symlink 只删链接本身
            freed += st.st_size
            count += 1
        except OSError:
            continue  # 单项失败跳过（可能被占用/权限），其余继续
    return freed, count


def file_empty_recycle(confirm=False):
    """清空回收站/废纸篓（内容被物理删除，不可恢复；Windows 走 Shell API，
    macOS/Linux 枚举内容目录逐项删除）。confirm=false 只查占用；
    用户明确确认后再 confirm=true 执行。三端均支持。"""
    if PLATFORM not in ("win32", "darwin", "linux"):
        return {"type": "error", "error": "清空回收站仅支持 Windows/macOS/Linux"}
    size, items = _query_recycle_bin()
    if not confirm:
        return {
            "type": "file_empty_recycle_preview",
            "executed": False,
            "size": size,
            "size_human": human_size(size) if size else "0 B",
            "item_count": items,
            "note": ("回收站/废纸篓当前占用如上（含所有应用与手动删除的文件）。清空会物理删除"
                     "其中的全部文件，不可恢复；经 file_trash 删除的记录清空后无法还原"
                     "（_pinvou_filemaster_trash 兜底方式不受影响）。用户明确确认后再以 confirm=true 执行。"),
        }
    if PLATFORM == "win32":
        try:
            import ctypes
            if ctypes.windll.shell32.SHEmptyRecycleBinW(None, None, SHERB_FLAGS) != 0:
                raise OSError("SHEmptyRecycleBinW 返回非零")
        except OSError as e:
            return {"type": "error", "error": "清空回收站失败: %s" % e}
        # 语义对齐：清空后 recycle-bin 记录已不在回收站 → 标记 erased
        with _log_lock:
            log = _read_trash_log()
            changed = False
            for e in log:
                if e.get("status") == "trashed" and e.get("via") == "recycle-bin":
                    e["status"] = "erased"
                    e["erased_at"] = _fmt_mtime(time.time())
                    changed = True
            if changed:
                _write_trash_log(log)
    else:
        freed, count = _empty_trash_dir()
        size, items = (freed, count) if freed else (size, items)
    return {
        "type": "file_empty_recycle_result",
        "executed": True,
        "freed": size or 0,
        "freed_human": human_size(size) if size else "0 B",
        "emptied_count": items or 0,
        "note": "回收站已清空（物理删除，不可恢复）。",
    }


# ── file_erase（物理删除，仅限 _pinvou_filemaster_trash 兜底区域）──────────────────────────

def _in_trash_container(path):
    """path 位于合法 trash 容器内：_pinvou_filemaster_trash 兜底（路径组件含 _pinvou_filemaster_trash）或系统废纸篓根之下。
    file_erase 的删除目标必须在此范围内——防止"日志落点的任意祖先"被整体擦除
    （如项目目录因内含 _pinvou_filemaster_trash 子目录而被整目录物理删除）。"""
    if "_pinvou_filemaster_trash" in _norm(path).split(os.sep):
        return True
    root = _system_trash_root()
    return bool(root) and _is_under(path, root)


def _log_has_trashed_dest(path):
    """日志准入：删除日志中存在 status=trashed、via 为兜底/系统废纸篓、dest 落点
    在该路径（或其下）**且 dest 本身位于合法 trash 容器内**的记录 → True。
    file_erase 只允许删本工具移入的备份（_pinvou_filemaster_trash / 系统废纸篓 / XDG Trash 内容）；
    日志是本工具删除的唯一事实来源，无记录一律拒绝。"""
    for e in _read_trash_log():
        if e.get("status") == "trashed" and e.get("via") in ("fallback-trash-dir", "system-trash"):
            dest = e.get("dest") or ""
            if dest and _in_trash_container(dest) and (
                    _norm(dest) == _norm(path) or _is_under(dest, path)):
                return True
    return False


def _mark_trash_log_erased(path):
    """把删除日志中 dest 落点在该 _pinvou_filemaster_trash 路径（或其下）的记录标记为 erased。
    物理删除审计：erased 记录不再被 file_restore list 列出，restore 时明确报错。"""
    with _log_lock:
        log = _read_trash_log()
        changed = False
        for e in log:
            if e.get("status") == "trashed" and e.get("via") in ("fallback-trash-dir", "system-trash"):
                dest = e.get("dest") or ""
                if dest and (_norm(dest) == _norm(path) or _is_under(dest, path)):
                    e["status"] = "erased"
                    e["erased_at"] = _fmt_mtime(time.time())
                    changed = True
        if changed:
            _write_trash_log(log)
        return changed


def _erase_item(path, allow_symlink=False):
    """单项目物理删除（worker 线程调用）。先全树预检 reparse point：
    Windows junction 在 rmtree 下会沿链接递归删除目标树（越界），永远拒绝；
    symlink 默认拒绝，allow_symlink=True（清空废纸篓/擦除备份）时只删链接本身。
    只读文件先加写权限再删。"""
    def _check_reparse_tree(p):
        st = os.stat(p, follow_symlinks=False)
        if stat.S_ISLNK(st.st_mode):
            if not allow_symlink:
                raise OSError("目标含 symlink，拒绝删除: %s" % p)
        elif getattr(st, "st_reparse_tag", None):
            raise OSError("目标含 reparse point（junction），拒绝删除: %s" % p)
        elif stat.S_ISDIR(st.st_mode):
            with os.scandir(p) as it:
                for e in it:
                    _check_reparse_tree(e.path)
    _check_reparse_tree(path)
    if os.path.isdir(path) and not os.path.islink(path):
        for root, dirs, files in os.walk(path):
            for d in dirs:
                try:
                    os.chmod(os.path.join(root, d), 0o777)
                except OSError:
                    pass
            for f in files:
                try:
                    os.chmod(os.path.join(root, f), 0o777)
                except OSError:
                    pass
        shutil.rmtree(path)
    else:
        try:
            os.chmod(path, 0o777)
        except OSError:
            pass
        os.remove(path)  # symlink 时仅删链接对象
    _mark_trash_log_erased(path)


def _erase_worker(task_id):
    """后台线程体：逐项物理删除（预检 reparse、chmod、rmtree/remove）+ 日志标记。
    绝不写 stdout——stdio JSON-RPC 流只能主线程写。"""
    with _tasks_lock:
        task = _tasks.get(task_id)
    if task is None:
        return
    results = []
    for item in task["items"]:
        if not item["allowed"]:
            results.append({"path": item["path"], "status": "rejected",
                            "error": item["rejected_reason"]})
        else:
            try:
                _erase_item(item["path"], allow_symlink=True)  # 备份内 symlink 只删链接本身
                _prune_empty_trash_container(item["path"])  # 清空后清理 _pinvou_filemaster_trash 容器（不留痕迹）
                results.append({"path": item["path"], "status": "erased"})
            except Exception as e:
                results.append({"path": item["path"], "status": "error", "error": str(e)})
        with _tasks_lock:
            task["done_count"] += 1
            task["results"] = list(results)  # 进度快照，主线程轮询时可见
    with _tasks_lock:
        task["results"] = results
        task["summary"] = {
            "total": task["total_count"],
            "erased": sum(1 for r in results if r["status"] == "erased"),
            "failed": sum(1 for r in results if r["status"] == "error"),
            "rejected": sum(1 for r in results if r["status"] == "rejected"),
            "total_size_human": task["summary"].get("total_size_human"),
        }
        task["finished_at"] = _fmt_mtime(time.time())
        task["finished_ts"] = time.time()
        task["status"] = "done"


def file_erase(paths=None, confirm=False):
    """物理删除（不可恢复），仅允许删除 file_trash 产生的 _pinvou_filemaster_trash 兜底目录内容
    （三重准入：白名单 + 路径组件含 _pinvou_filemaster_trash + **删除日志中有对应 dest 记录**）。
    confirm=false 只预览；用户明确确认后 confirm=true **后台异步执行**（大目录
    rmtree 可能超 30 秒底座超时），返回 task_id，用 file_trash_status 轮询。"""
    if not isinstance(paths, list) or not paths:
        return {"error": "paths 必须是非空绝对路径数组"}
    if len(paths) > TRASH_MAX_PATHS:
        return {"error": "paths 最多 %d 条，请分批处理" % TRASH_MAX_PATHS}
    home = os.path.expanduser("~")
    protected = _protected_roots()

    items = []
    total_bytes = 0
    for raw in paths:
        p, reason = _validate_trash_target(raw, home, protected, allow_trash_root=True)
        if reason:
            items.append({"path": raw, "allowed": False, "rejected_reason": reason,
                          "size": None, "size_human": None})
            continue
        if not _in_trash_container(p):
            items.append({"path": p, "allowed": False,
                          "rejected_reason": "仅支持删除 file_trash 移入的备份（_pinvou_filemaster_trash 兜底 / 系统废纸篓 / XDG Trash 内容）",
                          "size": None, "size_human": None})
            continue
        if not _log_has_trashed_dest(p):
            items.append({"path": p, "allowed": False,
                          "rejected_reason": "不在本工具删除日志中（仅允许删除 file_trash 移入的备份："
                                             "_pinvou_filemaster_trash 兜底 / 系统废纸篓 / XDG Trash 内容）",
                          "size": None, "size_human": None})
            continue
        size, estimated = _preview_size(p)
        total_bytes += size
        items.append({"path": p, "allowed": True, "rejected_reason": None,
                      "size": size, "size_human": ("约 " if estimated else "") + human_size(size)})

    allowed_count = sum(1 for i in items if i["allowed"])
    summary = {"total": len(items), "allowed": allowed_count,
               "rejected": len(items) - allowed_count,
               "total_size": total_bytes, "total_size_human": human_size(total_bytes)}

    if not confirm:
        return {
            "type": "file_erase_preview",
            "executed": False,
            "items": items,
            "summary": summary,
            "note": ("预览未执行，未改动任何文件。物理删除不可恢复（_pinvou_filemaster_trash 中的备份将永久消失，"
                     "对应删除日志记录也会失效）；请把清单展示给用户，明确确认后"
                     "用相同 paths + confirm=true 重调执行。"),
        }

    # 异步提交：后台线程逐项物理删除（大目录 rmtree 可能远超 30 秒底座超时）
    task_id = _next_task_id()  # 锁外生成（_next_task_id 内部持锁，Lock 不可重入）
    with _tasks_lock:
        active = sum(1 for t in _tasks.values() if t["status"] == "running")
        if active >= TRASH_MAX_ACTIVE_TASKS:
            return {"error": "已有 %d 个删除任务在运行中，请先用 file_trash_status 查看/等待"
                    % active}
        task = {
            "task_id": task_id,
            "kind": "erase",
            "status": "running",
            "created_at": _fmt_mtime(time.time()),
            "items": items,
            "done_count": 0,
            "total_count": len(items),
            "results": [],
            "summary": summary,
        }
        _tasks[task_id] = task
    threading.Thread(target=_erase_worker, args=(task_id,), daemon=True).start()
    return {
        "type": "file_erase_submitted",
        "task_id": task_id,
        "items": items,
        "summary": summary,
        "note": ("物理删除已在后台执行（不可恢复）。用 file_trash_status(task_id=%s) "
                 "轮询直到 status=done，再向用户汇报逐项结果；对应删除日志记录将标记 erased，"
                 "file_restore 不再列出。" % task_id),
    }


# ── 工具定义 ─────────────────────────────────────────────────────────────

TOOL_DEFS = [
    {
        "name": "file_find",
        "description": (
            "按文件名/目录名在本机搜索文件（大小写不敏感；文件与目录名都参与匹配）。"
            "**query 可选**：留空 + extensions/min_size_mb 等过滤 = 纯类型搜索（如找所有 .exe/.msi "
            "安装包：query 留空 + extensions=[\"exe\",\"msi\"]）——但必须有至少一个过滤条件，"
            "防止返回全盘。query 支持空格/标点分词，**多词为 AND（文件名需同时包含所有词）**，"
            "如 \"report final\"；要找一类文件请用 extensions，**不要把多个候选词一起传**"
            "（\"install setup\" 会因 AND 全命中而必 miss）。"
            "默认按概率序搜索 Desktop/Documents/Downloads/Pictures/Videos/Music，"
            "再对用户主目录整体兜底（含 AppData 下应用目录）；**默认范围只在主目录"
            "（Windows C:\\Users\\<用户名>），其他盘（D:\\、E:\\ 等）和任意位置一律用 "
            "dir 定向搜**（如 dir=\"D:\\\\\" 搜 D 盘根、dir=\"D:\\\\myWork\" 定向项目目录）；"
            "用户没给位置但目录名像项目/工作目录时，先用 dir=\"<盘符>:\\\\\" 定向常见盘符根重搜。"
            "最多 8 秒或命中 limit 即停（全盘类型搜索可能不全，可用 dir 定向）。"
            "返回 total_hits=收集到的全部命中数：total_hits 大于 count 说明被 limit 截断，"
            "应加 limit 或 dir 定向重搜，不要分段穷举。"
            "默认按相关度排序（全词>前缀>子串，同分按修改时间），可用 sort_by/order 改排序。"
            "可选过滤：extensions 按扩展名（目录不参与）；**用户提到时间（上周/最近几天/昨天）→ "
            "用 modified_after（不早于该日）/ modified_before（早于该日），YYYY-MM-DD 格式**；"
            "min_size_mb/max_size_mb 按大小（MB，只作用于文件）；exclude_dirs 额外排除同名目录。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string",
                          "description": "文件名关键词（可选；空格/标点分词，多词 AND 全命中；留空=纯类型搜索需配过滤条件）"},
                "limit": {"type": "integer", "default": 20,
                          "description": "最多返回条数，默认 20，上限 50"},
                "dir": {"type": "string",
                        "description": "可选绝对路径：定向只搜该目录（用户给了大致位置时用）"},
                "extensions": {"type": "array", "items": {"type": "string"},
                               "description": "可选扩展名过滤（如 [\"xlsx\",\"docx\"]，大小写不敏感）"},
                "modified_after": {"type": "string",
                                   "description": "可选：修改时间不早于。支持 YYYY-MM-DD（含当日）或相对天数 Nd（如 7d=最近 7 天含今天）。用户说\"上周/最近几天/昨天\"→ 直接用 Nd（7d/3d/1d），**不要自己推算日期**（模型推算日期会出错，本机按今天计算）"},
                "modified_before": {"type": "string",
                                    "description": "可选：修改时间早于。支持 YYYY-MM-DD（不含当日）或相对天数 Nd（如 30d=30 天前之前）"},
                "min_size_mb": {"type": "number",
                                "description": "可选：文件大小下限（MB，只作用于文件）"},
                "max_size_mb": {"type": "number",
                                "description": "可选：文件大小上限（MB，只作用于文件）"},
                "sort_by": {"type": "string", "enum": ["relevance", "mtime", "modified", "size", "name"],
                            "default": "relevance",
                            "description": "排序方式：relevance=相关度（默认，全词>前缀>子串，同分按修改时间）；mtime/modified=按修改时间；size=大小（目录恒排最后）；name=名称"},
                "order": {"type": "string", "enum": ["desc", "asc"], "default": "desc",
                          "description": "排序方向：desc=降序（默认）；asc=升序（relevance 时同分仍按修改时间从新到旧）"},
                "exclude_dirs": {"type": "array", "items": {"type": "string"},
                                 "description": "可选：额外排除的同名目录（如 [\"node_modules\",\"dist\"]）"},
            },
            "required": [],
        },
    },
    {
        "name": "disk_scan",
        "description": (
            "只读磁盘占用分析，双模式：① 不传 path＝概览：扫描常见积聚地（%TEMP%、"
            "Downloads、AppData、微信/钉钉/飞书、Chrome/Edge、开发缓存、Program Files 等），"
            "每组给大小/文件数/🟢🟡🔴 风险/说明，附 >500MB 大文件与磁盘总容量；"
            "② 传 path＝下钻：列该目录直接子项按大小降序 Top20。"
            "推荐逐层下钻定位大文件夹：先概览，再对最大的组用 path 逐层进入。"
            "全程只读；统计目录优先用本工具，exec_shell 仅作兜底且须过滤限量。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {"type": "string",
                         "description": "可选绝对路径：下钻模式，列该目录的直接子项（Top20 按大小降序）"},
                "refresh": {"type": "boolean", "default": False,
                            "description": "兼容参数；本工具无状态，每次调用都实时重扫"},
            },
            "required": [],
        },
    },
    {
        "name": "file_trash",
        "description": (
            "把文件/目录移入系统回收站（Windows）/ 废纸篓（macOS ~/.Trash）/ XDG Trash（Linux），可恢复，绝不物理删除。内置白名单：系统目录、"
            "Program Files、ProgramData、盘符根、用户主目录本身、pinvou3 目录一律硬拒绝。"
            "confirm=false（默认）只返回预览清单；用户明确确认后用 confirm=true 提交——"
            "提交后删除在后台执行（避免大目录阻塞），返回 task_id，必须用 "
            "file_trash_status(task_id=...) 轮询直到 status=done，再向用户汇报逐项结果。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "paths": {"type": "array", "items": {"type": "string"},
                          "description": "待删除的绝对路径数组（逐项核对后的明确清单，不接受通配符）"},
                "confirm": {"type": "boolean", "default": False,
                            "description": "false=只预览不执行（默认）；用户明确确认后传 true 提交后台删除"},
            },
            "required": ["paths"],
        },
    },
    {
        "name": "file_trash_status",
        "description": (
            "查询 file_trash 提交的后台删除任务：有 task_id 查单个（running 含已完成进度"
            "done_count/total_count，done 含逐项结果与汇总）；无 task_id 列最近任务。"
            "file_trash(confirm=true) 提交后必须用它轮询到 status=done 再汇报。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string",
                            "description": "可选：file_trash 提交时返回的 task_id"},
                "limit": {"type": "integer", "default": 10,
                          "description": "无 task_id 时最多列出的任务数，上限 20"},
            },
            "required": [],
        },
    },
    {
        "name": "file_empty_recycle",
        "description": (
            "清空回收站/废纸篓（内容被物理删除，不可恢复；经 file_trash 删除的记录清空后将无法"
            "还原）。Windows 走 Shell API、macOS/Linux 枚举内容目录逐项删除。confirm=false"
            "（默认）只查询占用；用户明确确认后以 confirm=true 执行。三端均支持。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "confirm": {"type": "boolean", "default": False,
                            "description": "false=只查占用不执行（默认）；用户明确确认后传 true 才清空"},
            },
            "required": [],
        },
    },
    {
        "name": "file_erase",
        "description": (
            "物理删除（不可恢复），**仅允许删除 file_trash 移入的备份**（_pinvou_filemaster_trash 兜底 / 系统废纸篓 /"
            "XDG Trash 内容，删除日志中有记录且位于 trash 容器内才允许）——用于兜底后真正释放空间。白名单同 file_trash"
            "（系统区域/盘符根/主目录硬拒），拒绝 reparse point。"
            "confirm=false（默认）只返回预览清单；用户明确确认后以 confirm=true 执行；"
            "执行后对应删除日志记录标记 erased，file_restore 不再列出。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "paths": {"type": "array", "items": {"type": "string"},
                          "description": "待物理删除的 _pinvou_filemaster_trash 内绝对路径数组（逐项核对，不接受通配符）"},
                "confirm": {"type": "boolean", "default": False,
                            "description": "false=只预览不执行（默认）；用户明确确认后传 true 才物理删除"},
            },
            "required": ["paths"],
        },
    },
    {
        "name": "file_restore",
        "description": (
            "误删恢复（与 file_trash 配套）：file_trash 删除时自动写本机日志，"
            "action='list' 从日志列出待恢复项（名称/原始路径/删除时间，不靠对话记忆）；"
            "action='restore' + path=<条目的 original_path> 精确还原到原位置"
            "（回收站方式走系统还原，_pinvou_filemaster_trash 兜底方式直接挪回）。"
            "注意：回收站被 file_empty_recycle 清空后，走 recycle-bin 的记录将无法还原。"
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "restore"], "default": "list",
                           "description": "list=列出待恢复项；restore=按原始路径还原"},
                "path": {"type": "string",
                         "description": "restore 必填：要还原条目的 original_path（来自 list 结果或删除时的原路径）"},
                "limit": {"type": "integer", "default": 20,
                          "description": "list 时最多返回条数，默认 20，上限 50"},
            },
            "required": [],
        },
    },
]

# ── JSON-RPC 2.0 协议处理 ────────────────────────────────────────────────

def _send(msg):
    sys.stdout.write(json.dumps(msg, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _result(req_id, result):
    _send({"jsonrpc": "2.0", "id": req_id, "result": result})


def _error(req_id, code, message):
    _send({"jsonrpc": "2.0", "id": req_id, "error": {"code": code, "message": message}})


def _call_tool(name, args):
    if name == "file_find":
        return file_find(query=args.get("query", ""),
                         limit=args.get("limit", 20),
                         dir=args.get("dir"),
                         extensions=args.get("extensions"),
                         modified_after=args.get("modified_after"),
                         modified_before=args.get("modified_before"),
                         min_size_mb=args.get("min_size_mb"),
                         max_size_mb=args.get("max_size_mb"),
                         sort_by=args.get("sort_by", "relevance"),
                         order=args.get("order", "desc"),
                         exclude_dirs=args.get("exclude_dirs"))
    if name == "disk_scan":
        return disk_scan(path=args.get("path"),
                         refresh=args.get("refresh", False))
    if name == "file_trash":
        return file_trash(paths=args.get("paths"),
                          confirm=args.get("confirm", False))
    if name == "file_trash_status":
        return file_trash_status(task_id=args.get("task_id"),
                                 limit=args.get("limit", 10))
    if name == "file_empty_recycle":
        return file_empty_recycle(confirm=args.get("confirm", False))
    if name == "file_erase":
        return file_erase(paths=args.get("paths"),
                          confirm=args.get("confirm", False))
    if name == "file_restore":
        return file_restore(action=args.get("action", "list"),
                            path=args.get("path"),
                            limit=args.get("limit", 20))
    return None


def _handle(msg):
    method = msg.get("method")
    req_id = msg.get("id")

    if req_id is None:
        return

    if method == "initialize":
        _result(req_id, {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "pinvou3-file-master", "version": "1.7.0"},
        })
    elif method == "ping":
        # MCP 标准保活；部分 SDK 客户端连上即发，不支持会被判协议错误
        _result(req_id, {})
    elif method == "tools/list":
        _result(req_id, {"tools": TOOL_DEFS})
    elif method == "tools/call":
        params = msg.get("params") or {}
        result = _call_tool(params.get("name"), params.get("arguments") or {})
        if result is None:
            _error(req_id, -32601, "unknown tool: %s" % params.get("name"))
            return
        _result(req_id, {
            "content": [{"type": "text", "text": json.dumps(result, ensure_ascii=False)}],
        })
    else:
        _error(req_id, -32601, "method not found: %s" % method)


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except Exception:
            continue
        try:
            _handle(msg)
        except Exception as e:
            rid = msg.get("id") if isinstance(msg, dict) else None
            if rid is not None:
                _error(rid, -32603, "internal error: %s" % e)


if __name__ == "__main__":
    main()
