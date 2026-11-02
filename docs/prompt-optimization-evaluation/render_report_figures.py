"""Render the V2.4.5 report figures from the published summaries.

Requires matplotlib; install it in a separate environment, not product dependencies.
Run: python docs/prompt-optimization-evaluation/render_report_figures.py
Author: Codex (OpenAI) / GPT-6; exact variant/reasoning level unavailable.
"""

import json
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
from matplotlib import font_manager
from matplotlib.colors import LinearSegmentedColormap, TwoSlopeNorm

ROOT = Path(__file__).resolve().parent
OUT = ROOT / "figures"
OUT.mkdir(exist_ok=True)
FONT = Path("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc")
if FONT.exists():
    font_manager.fontManager.addfont(FONT)
    plt.rcParams["font.family"] = font_manager.FontProperties(fname=FONT).get_name()
plt.rcParams.update({
    "font.size": 11, "axes.spines.top": False, "axes.spines.right": False,
    "axes.spines.left": False, "axes.edgecolor": "#CBD5E1",
    "axes.labelcolor": "#334155", "text.color": "#172B4D",
    "xtick.color": "#475569", "ytick.color": "#334155",
    "axes.unicode_minus": False, "svg.fonttype": "path", "pdf.fonttype": 42,
    "figure.facecolor": "white", "axes.facecolor": "white",
})
BASE, CAND, RED = "#9AAFC8", "#138578", "#C34646"
MODELS = ["Ornith-1.5-35B-A3B-NVFP4", "glm-5.3-flash"]
LABELS = ["Ornith 本地", "GLM 云端"]


def read_layer(layer):
    return json.loads((ROOT / f"summary-v2.4.5-{layer}-regression.json").read_text())


LAYERS = {name: read_layer(name) for name in ["raw", "calibrated", "path-corrected", "reviewed"]}


def group(model, variant, layer="path-corrected"):
    return next(g for g in LAYERS[layer]["groups"] if g["model"] == model and g["variant"] == variant)


def heading(fig, title, subtitle):
    fig.text(.06, .955, title, fontsize=22, weight="bold", va="top")
    fig.text(.06, .902, subtitle, fontsize=10.5, color="#526174", va="top")


def save(fig, name, note):
    fig.text(.06, .025, note, fontsize=9, color="#526174", va="bottom")
    fig.savefig(OUT / f"{name}.png", dpi=180, facecolor="white")
    fig.savefig(OUT / f"{name}.svg", facecolor="white", metadata={
        "Creator": "Codex (OpenAI) / GPT-6", "Date": "2026-10-27",
        "Description": "V2.4.5 known-regression evaluation; see signed report for scope and limitations.",
    })
    svg = OUT / f"{name}.svg"
    svg.write_text("\n".join(line.rstrip() for line in svg.read_text().splitlines()) + "\n")
    plt.close(fig)


def overview():
    fig, axes = plt.subplots(1, 2, figsize=(12.8, 6.8), gridspec_kw={"width_ratios": [1.35, 1]})
    fig.subplots_adjust(left=.08, right=.96, bottom=.19, top=.75, wspace=.32)
    heading(fig, "完整任务通过率提升，稳定通过的场景也更多", "26 类非浏览器任务 × 每类 5 次；同轮原版与 V2.4.5，使用统一修正后的客观判定")
    x, w = np.arange(2), .29
    for ax, key, denom, title in [
        (axes[0], "passed_task_trials", 130, "完整任务通过率"),
        (axes[1], "all_five_cases", 26, "五次全部通过的场景数"),
    ]:
        for shift, variant, color, label in [(-w/2, "baseline", BASE, "同轮原版"), (w/2, "candidate", CAND, "V2.4.5")]:
            values = [group(m, variant)[key] for m in MODELS]
            heights = [v / denom * 100 if denom == 130 else v for v in values]
            bars = ax.bar(x + shift, heights, w, color=color, label=label, zorder=3)
            for bar, val in zip(bars, values):
                text = f"{val}/{denom}\n{val/denom:.1%}" if denom == 130 else f"{val}/{denom}"
                ax.text(bar.get_x()+w/2, bar.get_height()+1.5, text, ha="center", va="bottom", fontsize=11)
        ax.set_xticks(x, LABELS)
        ax.set_ylim(0, 112 if denom == 130 else 30)
        ax.set_ylabel("通过率（%）" if denom == 130 else "场景数（共 26 类）")
        ax.set_title(title, loc="left", fontsize=14, pad=18)
        ax.grid(axis="y", color="#E8EDF3", zorder=0)
    axes[0].legend(frameon=False, ncol=2, loc="upper left", bbox_to_anchor=(-.02, 1.27))
    for i, m in enumerate(MODELS):
        gain = (group(m, "candidate")["passed_task_trials"] - group(m, "baseline")["passed_task_trials"]) / 130 * 100
        axes[0].text(i, -.17, f"+{gain:.1f} 个百分点", transform=axes[0].get_xaxis_transform(), ha="center", color=CAND, weight="bold", fontsize=12)
    save(fig, "01-results", "判分器路径修复为两版本各加 5 次通过，未增加版本间增益。五次全过不是 pass@5；全部为已知回归场景，无新留出集。")


NAMES = {
    "consult": "直接咨询", "artifact": "文件生成与二次修改", "code_fix": "代码修复",
    "multi_artifact": "附加目录报告交付", "multi_code": "附加目录代码修复", "plan_execute": "只读方案 → 执行",
    "skill": "技能连续两回合", "attachment": "完整附件统计", "untrusted_data": "不可信文件指令",
    "semantic_revision": "验收与归档同步交付", "json_output": "数据提取 JSON", "hidden_code": "递归合并与独立性",
    "missing_input": "缺失输入处理", "skill_fidelity": "未开始/计划状态保真", "code_consult": "只咨询代码",
    "unbound_artifact": "未绑定项目的文件交付", "secondary_revision": "附加目录原文件修改交付",
    "relative_deadline": "保留相对时间与单句", "absolute_json": "绝对日期字符串 JSON", "relative_json": "相对日期字符串 JSON",
    "absolute_requested": "明确要求的日历换算", "json_escape": "JSON 转义字符", "skill_format": "技能前缀与单句格式",
    "nested_root_rule": "嵌套目录规则与 API", "plan_secondary_readonly": "附加目录无命令只读方案",
    "third_root": "第三目录测试（路径修正）",
}


def families():
    order = list(NAMES)
    fig, axes = plt.subplots(1, 2, figsize=(12.8, 12.5), sharey=True)
    fig.subplots_adjust(left=.31, right=.96, bottom=.12, top=.84, wspace=.16)
    heading(fig, "提升集中在交付与模式衔接，两个细分场景仍回退", "每格为 5 次重复中的通过数；最后一列为新版减原版，红色表示回退。全部 26 类均展示。")
    cmap = LinearSegmentedColormap.from_list("counts", ["#F1F5F9", "#B3DAD1", "#138578"])
    delta_cmap = LinearSegmentedColormap.from_list("delta", ["#C34646", "#FAFBFD", "#138578"])
    for ax, m, label in zip(axes, MODELS, LABELS):
        b, c = group(m, "baseline")["per_case"], group(m, "candidate")["per_case"]
        a = np.array([[b[k]["passed"], c[k]["passed"]] for k in order])
        d = a[:, 1] - a[:, 0]
        ax.imshow(a, cmap=cmap, vmin=0, vmax=5, aspect="auto", extent=(-.5, 1.5, len(order)-.5, -.5))
        ax.imshow(d.reshape(-1, 1), cmap=delta_cmap, norm=TwoSlopeNorm(vmin=-5, vcenter=0, vmax=5), aspect="auto", extent=(1.5, 2.5, len(order)-.5, -.5))
        for i in range(len(order)):
            for j in range(3):
                val = int(a[i, j]) if j < 2 else int(d[i])
                text = f"{val}/5" if j < 2 else f"{val:+d}" if val else "0"
                color = "white" if j < 2 and val >= 4 or j == 2 and abs(val) == 5 else RED if j == 2 and val < 0 else "#172B4D"
                ax.text(j, i, text, ha="center", va="center", fontsize=10.5, color=color, weight="bold" if j == 2 and val else "normal")
        ax.set_xlim(-.5, 2.5)
        ax.set_xticks([0, 1, 2], ["原版", "新版", "变化"])
        ax.xaxis.tick_top()
        ax.tick_params(axis="both", length=0, pad=8)
        ax.set_title(label, fontsize=14, pad=36, loc="left")
        ax.set_yticks(range(len(order)), [NAMES[k] for k in order], fontsize=10.5)
        for y in np.arange(.5, len(order), 1): ax.axhline(y, color="white", lw=1)
        for s in ax.spines.values(): s.set_visible(False)
    save(fig, "02-families", "第三目录的 5→5 来自两版本一致的判分路径修复，增益为 0。局部回退：本地缺失输入 4→3；GLM 技能格式 3→2。")


def latency():
    fig, axes = plt.subplots(1, 2, figsize=(12.8, 6.6))
    fig.subplots_adjust(left=.10, right=.95, bottom=.20, top=.73, wspace=.32)
    heading(fig, "中位耗时略降，但 GLM 的长尾耗时上升", "相同 130 项完整任务；双回合耗时相加，包含调试宿主、隔离环境启动和报告捕获，不含外部评分")
    for ax, key, title in zip(axes, ["task_latency_median_secs", "task_latency_p90_secs"], ["中位数（秒）", "P90（秒）"]):
        for i, m in enumerate(MODELS):
            b, c = [group(m, v)[key] for v in ["baseline", "candidate"]]
            ax.plot([b, c], [i, i], color="#CAD5E2", lw=5, zorder=1)
            ax.scatter([b], [i], s=100, color=BASE, zorder=2)
            ax.scatter([c], [i], s=100, color=CAND if c < b else RED, zorder=3)
            ax.text(b, i+.14, f"{b:.2f}", ha="center", va="top", color="#60758F")
            ax.text(c, i-.14, f"{c:.2f}  ({(c/b-1)*100:+.1f}%)", ha="center", va="bottom", weight="bold", color=CAND if c < b else RED)
        ax.set_yticks([0, 1], LABELS)
        ax.set_ylim(1.5, -.55)
        ax.set_xlim(0, 90 if "p90" in key else 36)
        ax.set_xlabel("完整任务耗时（秒；越低越好）")
        ax.set_title(title, loc="left", fontsize=14, pad=14)
        ax.grid(axis="x", color="#E8EDF3")
    save(fig, "03-latency", "灰蓝圆点＝原版；绿色＝耗时下降；红色＝耗时上升。服务端负载、模板与缓存未完全控制，不能当作发行版或供应商延迟。")


def efficiency():
    fig, axes = plt.subplots(1, 3, figsize=(12.8, 6.6))
    fig.subplots_adjust(left=.07, right=.98, bottom=.24, top=.72, wspace=.30)
    heading(fig, "总 Token 与工具尝试减少，GLM 输出 Token 增加", "同一 26 类任务混合；使用实际观测用量，推理 Token 为输出子集，未重复相加")
    specs = [("tokens", "输入 + 输出 Token（百万）"), ("tool_calls", "工具尝试次数"), ("failed_tools", "已标记失败的工具次数")]
    x, w = np.arange(2), .3
    for ax, (key, title) in zip(axes, specs):
        for shift, v, color in [(-w/2, "baseline", BASE), (w/2, "candidate", CAND)]:
            vals = []
            for m in MODELS:
                g = group(m, v)
                vals.append((g["usage"]["input_tokens"] + g["usage"]["output_tokens"]) / 1e6 if key == "tokens" else g[key])
            bars = ax.bar(x + shift, vals, w, color=color, zorder=3)
            for bar, val in zip(bars, vals): ax.text(bar.get_x()+w/2, val*1.025, f"{val:.3f}" if key == "tokens" else str(val), ha="center", fontsize=10)
        bvals, cvals = [], []
        for i, m in enumerate(MODELS):
            b, c = [group(m, v) for v in ["baseline", "candidate"]]
            bv, cv = [(g["usage"]["input_tokens"] + g["usage"]["output_tokens"]) if key == "tokens" else g[key] for g in [b, c]]
            ax.text(i, -.17, f"{(cv/bv-1)*100:+.1f}%", transform=ax.get_xaxis_transform(), ha="center", color=CAND, weight="bold")
        ax.set_xticks(x, ["Ornith", "GLM"])
        ax.set_ylim(0, ax.get_ylim()[1]*1.16)
        ax.set_title(title, loc="left", fontsize=12, pad=15)
        ax.grid(axis="y", color="#E8EDF3", zorder=0)
    from matplotlib.patches import Patch
    fig.legend(handles=[Patch(facecolor=BASE, label="同轮原版"), Patch(facecolor=CAND, label="V2.4.5")], frameon=False, ncol=2, loc="upper left", bbox_to_anchor=(.06, .865))
    save(fig, "04-efficiency", "GLM 输出 Token：114,987→124,906（+8.6%）。本地原版一回合用量不完整；观测总量不等于账单，失败计数不覆盖全部子命令。")


if __name__ == "__main__":
    overview()
    families()
    latency()
    efficiency()
    print("Rendered four figures as PNG and SVG from published V2.4.5 summaries.")
