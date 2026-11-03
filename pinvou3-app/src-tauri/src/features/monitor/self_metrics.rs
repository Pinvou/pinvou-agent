//! app 侧自测推理指标累加器。
//!
//! 职责边界——本模块只管「推理通路 TTFT / 生成速度 / 累计 tokens / KV 命中率」
//! 累加与读出（[`SelfMetrics`]），不涉及系统资源采集 (CPU/GPU/内存)与模型探针
//! （见 [`super::model_probe`]）。打点入口由 engine forwarder 在 TurnStarted /
//! 首个 MessageDelta / TurnUsage / ToolCallStarted / TurnComplete 五处调用;
//! 读出由 facade `sample_all` 经 `MonitorState` 触发。
//!
//! 计时源语义：TTFT / TPS 优先采用引擎 `TurnUsage` 事件携带的
//! `first_token_ms` / `request_ms`——引擎在请求派发点起表(含真实网络首字节,
//! 首 token 以首个内容事件计,思考 delta 也算),是权威源。完整对按模型调用
//! 逐条累加:TTFT 取首条完整对,生成窗口与该调用的 output_tokens 成对累加,
//! 多调用轮(续跑/reprompt)窗口与逐条配对的 tokens 同源求和;墙钟 `Instant`
//! 重测发生在转发通路上,含事件分发延迟,仅作引擎未上报任何完整对时的回落
//! (usage 未上报 / 非流式 consult / 无内容流)。两组计时不同源不混拼。
//! 「分子分母同源」以引擎分子非零为前提:provider 只报输入不报输出时引擎
//! 分子为 0,收尾分子回落轮级 usage、再退字符估算,分母仍取引擎窗口。
//! 统计范围:引擎内 subagent worker 与自动压缩摘要的调用不走流式通路、
//! 不发 `TurnUsage`(usage 走成本侧通道),不在本累加内。

use std::collections::HashMap;
use std::time::Instant;

use parking_lot::Mutex;
use serde::Serialize;

/// app 侧自测指标的对外快照（单调累计，进程生命周期）。字段与 vLLM `/metrics`
/// 同"sum+count"形状，好让监控页「按住清除」的区间重算逻辑原样复用。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SelfPerfSnapshot {
    /// 首字延迟：Σ(TTFT) / 次数，仅纯文本轮（无工具调用）计入。
    pub ttft_sum_s: f64,
    pub ttft_count: u64,
    /// 生成速度：tps_tokens / tps_time_s = tok/s。同样仅纯文本轮计入
    /// （D2 维持跳过工具轮：工具轮的"首 token"是不上屏的工具调用 JSON，
    /// TTFT 与面板「首字」语义错位，整轮跳过保持口径稳定）。
    pub tps_tokens: u64,
    pub tps_time_s: f64,
    /// 累计 tokens：**全部轮**（含工具轮）真实 usage 之和。
    pub gen_tokens_total: u64,
    pub prompt_tokens_total: u64,
    /// KV 命中率（token 口径）：cache_hit /(hit+miss)×100。来自 usage 的
    /// prompt_cache_hit/miss_tokens——DeepSeek/部分云端会返回，返回不了的后端保持 0。
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
}

#[derive(Debug, Default)]
struct SelfPerfInner {
    ttft_sum_s: f64,
    ttft_count: u64,
    tps_tokens: u64,
    tps_time_s: f64,
    gen_tokens_total: u64,
    prompt_tokens_total: u64,
    cache_hit_tokens: u64,
    cache_miss_tokens: u64,
}

/// 单个 session 在途轮次的计时状态（TTFT 需要"起始"与"首 token"两个时点）。
#[derive(Debug)]
struct TurnTiming {
    start: Instant,
    first: Option<Instant>,
    had_tool: bool,
    output_chars: u64,
    /// 引擎 `TurnUsage` 上报的权威计时，按「完整对」逐模型调用累加：
    /// `engine_ttft_ms` 取首条完整对的 first_token_ms（本 turn 首个内容 token），
    /// 生成窗口（request_ms − first_token_ms，同钟饱和相减）与该调用的
    /// output_tokens 只在窗口 > 0 时成对累加——TPS 分子分母严格同源，无工具
    /// 多调用轮（续跑/reprompt）如实求和。半对（None 混 Some）整条不落，
    /// 杜绝跨调用拼字段；`engine_ttft_ms` 为 None 表示引擎本轮未上报任何
    /// 完整对（收尾整组回落墙钟）。
    engine_ttft_ms: Option<u64>,
    engine_gen_window_ms: u64,
    engine_output_tokens: u64,
}

/// app 侧自测推理指标累加器。流式转发通路（engine.rs forwarder）在
/// TurnStarted / 首个 MessageDelta / TurnUsage / ToolCallStarted / TurnComplete
/// 五处打点写入，`sample_all` 读出。`inflight` 按 `session_id` 键控——多 session
/// 并发各测各的，不串台；`perf` 是全局单调累计，各轮把自己的增量加进去。
#[derive(Debug, Default)]
pub struct SelfMetrics {
    perf: Mutex<SelfPerfInner>,
    inflight: Mutex<HashMap<String, TurnTiming>>,
}

impl SelfMetrics {
    /// TurnStarted：打点本轮起始。覆盖任何残留（上一轮异常未收尾）。
    pub fn on_turn_started(&self, session_id: &str) {
        self.inflight.lock().insert(
            session_id.to_string(),
            TurnTiming {
                start: Instant::now(),
                first: None,
                had_tool: false,
                output_chars: 0,
                engine_ttft_ms: None,
                engine_gen_window_ms: 0,
                engine_output_tokens: 0,
            },
        );
    }

    /// 首个 MessageDelta：记首 token 时点（仅首次）。TTFT 的停表点 + 生成时长起点。
    #[cfg(test)]
    pub fn on_first_delta(&self, session_id: &str) {
        self.on_message_delta(session_id, 0);
    }

    pub fn on_message_delta(&self, session_id: &str, char_count: usize) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            if t.first.is_none() {
                t.first = Some(Instant::now());
            }
            t.output_chars = t.output_chars.saturating_add(char_count as u64);
        }
    }

    /// 引擎 `TurnUsage` 事件：按完整对逐模型调用累加权威计时。TurnUsage 每次
    /// 模型调用发一条——纯文本轮通常一条，续跑/reprompt 等无工具多调用轮会有
    /// 多条（各带各的对），带工具的轮收尾整体跳过 TTFT/TPS（D2 不变）。只有
    /// 同时携带完整 `(first_token_ms, request_ms)` 的事件才落值：TTFT 取首条
    /// 完整对（后续调用不覆盖），生成窗口与该调用的 output_tokens 逐条成对
    /// 累加（窗口为 0 的对不计入 TPS，保持「tokens 必须带非零时长」口径），
    /// 半对不落——分子分母永远同源同条，杜绝用两次调用的字段拼对；收尾时若
    /// 一条完整对都没有，整组回落墙钟。
    pub fn on_engine_usage(
        &self,
        session_id: &str,
        first_token_ms: Option<u64>,
        request_ms: Option<u64>,
        output_tokens: u32,
    ) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            // (Some, Some) 才是完整对(与 Option::zip 同判);元组解构以便同取两值算窗口。
            if let (Some(ttft_ms), Some(req_ms)) = (first_token_ms, request_ms) {
                if t.engine_ttft_ms.is_none() {
                    t.engine_ttft_ms = Some(ttft_ms);
                }
                let window_ms = req_ms.saturating_sub(ttft_ms);
                if window_ms > 0 {
                    t.engine_gen_window_ms += window_ms;
                    t.engine_output_tokens += output_tokens as u64;
                }
            }
        }
    }

    /// 本轮出现过工具调用 → 标记。收尾时据此跳过 TTFT/TPS（D2）。
    pub fn on_tool(&self, session_id: &str) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            t.had_tool = true;
        }
    }

    /// TurnComplete：用精确 usage 累加。tokens/KV 永远记；TTFT/TPS 仅跳过工具轮。
    /// 部分远端模型会省略 output_tokens，TPS 会回落到流式文本字符数估算。
    pub fn on_turn_complete(
        &self,
        session_id: &str,
        input_tokens: u32,
        output_tokens: u32,
        cache_hit: Option<u32>,
        cache_miss: Option<u32>,
    ) {
        let timing = self.inflight.lock().remove(session_id);
        let mut p = self.perf.lock();
        p.gen_tokens_total += output_tokens as u64;
        p.prompt_tokens_total += input_tokens as u64;
        if let Some(h) = cache_hit {
            p.cache_hit_tokens += h as u64;
        }
        if let Some(m) = cache_miss {
            p.cache_miss_tokens += m as u64;
        }
        if let Some(t) = timing {
            if !t.had_tool {
                // 计时源优先级：引擎 TurnUsage 字段是权威源 (first_token_ms 从
                // 请求派发点起表,含真实网络首字节;request_ms 同钟收束于流结束,
                // 两者相减即纯生成窗口)。无工具轮的多次模型调用各自成对累加窗口
                // 与 per-call tokens,收尾求和——TPS 是跨调用真实均速,引擎分子
                // 非零时分子分母同源(引擎分子为 0 时分子回落,见下方)。引擎一条
                // 完整对都没上报时(usage 未上报 / 非流式 consult / 无内容流)
                // 回落墙钟 Instant 重测;TTFT 与生成时长必须同源,不混拼两套时钟。
                // 被取消的纯文本轮:TurnUsage 先于取消检查发出,截断的窗口与
                // tokens 照常计入(真实部分测量,与原墙钟路径同形)。
                let (ttft_s, gen_s) = match t.engine_ttft_ms {
                    Some(ttft_ms) => {
                        let ttft = ttft_ms as f64 / 1000.0;
                        (Some(ttft), Some(t.engine_gen_window_ms as f64 / 1000.0))
                    }
                    None => match t.first {
                        Some(first) => (
                            Some(first.duration_since(t.start).as_secs_f64()),
                            Some(first.elapsed().as_secs_f64()),
                        ),
                        None => (None, None),
                    },
                };
                if let (Some(ttft), Some(gen_s)) = (ttft_s, gen_s) {
                    p.ttft_sum_s += ttft;
                    p.ttft_count += 1;
                    // TPS 分子：优先取与窗口逐条配对的引擎 per-call tokens；
                    // 个别 provider 只报输入不报输出（引擎分子为 0）时回落轮级
                    // usage，再退字符估算（与墙钟路径共用口径）。
                    let tps_units = if t.engine_output_tokens > 0 {
                        t.engine_output_tokens
                    } else if output_tokens > 0 {
                        output_tokens as u64
                    } else {
                        // Some remote providers stream text but omit final token usage.
                        // Use a conservative character-based fallback so speed still
                        // reflects completed pure text turns instead of staying blank.
                        (t.output_chars / 2).max(1)
                    };
                    if gen_s > 0.0 && tps_units > 0 {
                        p.tps_time_s += gen_s;
                        p.tps_tokens += tps_units;
                    }
                }
            }
        }
    }

    /// Turn aborted (stopped / session reclaimed, never reached TurnComplete):
    /// remove the inflight entry so the session's TurnTiming cannot linger.
    /// No perf accumulation happens for an aborted turn.
    pub fn on_turn_aborted(&self, session_id: &str) {
        self.inflight.lock().remove(session_id);
    }

    /// Session deleted: clear the session's leftover inflight entry.
    /// Idempotent (HashMap::remove).
    pub fn drop_session(&self, session_id: &str) {
        self.inflight.lock().remove(session_id);
    }

    pub fn snapshot(&self) -> SelfPerfSnapshot {
        let p = self.perf.lock();
        SelfPerfSnapshot {
            ttft_sum_s: p.ttft_sum_s,
            ttft_count: p.ttft_count,
            tps_tokens: p.tps_tokens,
            tps_time_s: p.tps_time_s,
            gen_tokens_total: p.gen_tokens_total,
            prompt_tokens_total: p.prompt_tokens_total,
            cache_hit_tokens: p.cache_hit_tokens,
            cache_miss_tokens: p.cache_miss_tokens,
        }
    }

    /// Test-only: number of inflight entries (the production snapshot carries
    /// no debug surface; the old `debug_snapshot`/`SelfMetricsDebugSnapshot`/
    /// `last_event` chain is gone).
    #[cfg(test)]
    fn debug_counts(&self) -> usize {
        self.inflight.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_metrics_aborted_turn_clears_inflight() {
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_aborted("s1");
        let inflight = m.debug_counts();
        assert_eq!(inflight, 0);
        // 中断轮不污染 perf 累计：不写 tokens、不计 TTFT/TPS。
        let s = m.snapshot();
        assert_eq!(s.gen_tokens_total, 0);
        assert_eq!(s.prompt_tokens_total, 0);
        assert_eq!(s.ttft_count, 0);
    }

    #[test]
    fn self_metrics_drop_session_clears_inflight() {
        let m = SelfMetrics::default();
        // s1 completes normally; s2's inflight marker is left unfinished.
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 10, 5, None, None);
        m.on_turn_started("s2");
        let inflight = m.debug_counts();
        assert_eq!(inflight, 1);
        // Removing s2: inflight should be cleared.
        m.drop_session("s2");
        let inflight = m.debug_counts();
        assert_eq!(inflight, 0);
        // drop_session 幂等：再删一次无副作用。
        m.drop_session("s2");
    }

    #[test]
    fn self_metrics_first_turn_per_session_records_ttft_tps() {
        // 监控面板需要首轮纯文本恢复后立刻显示 TTFT/TPS；仅工具轮跳过。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_first_delta("s1"); // 幂等
        m.on_turn_complete("s1", 100, 50, Some(90), Some(10));
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert_eq!(s.tps_tokens, 50);
        assert_eq!(s.gen_tokens_total, 50); // tokens 照记
        assert_eq!(s.prompt_tokens_total, 100);
        assert_eq!(s.cache_hit_tokens, 90); // cache 照记
        assert_eq!(s.cache_miss_tokens, 10);
    }

    #[test]
    fn self_metrics_second_pure_turn_records_ttft_tps() {
        let m = SelfMetrics::default();
        // 首轮也记
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 10, 5, None, None);
        // 二轮继续记
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 100, 50, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 2);
        assert_eq!(s.tps_tokens, 55);
        assert!(s.tps_time_s > 0.0);
        assert_eq!(s.gen_tokens_total, 55); // 两轮 tokens 都在
    }

    #[test]
    fn self_metrics_tool_turn_skips_ttft_tps_but_keeps_tokens() {
        let m = SelfMetrics::default();
        // 先跑两轮纯文本(都计 TTFT)
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 1, 1, None, None);
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 1, 1, None, None);
        // 工具轮:tokens 记,TTFT/TPS 跳(D2)
        m.on_turn_started("s1");
        m.on_tool("s1");
        m.on_first_delta("s1");
        m.on_turn_complete("s1", 200, 80, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 2); // 工具轮没加,仍是前两轮
        assert_eq!(s.gen_tokens_total, 82); // 1+1+80 全记
    }

    #[test]
    fn self_metrics_delta_without_start_still_counts_tokens() {
        // forwarder 中途起来、没接到 TurnStarted 的轮:TTFT 记不了,但 tokens 不丢。
        let m = SelfMetrics::default();
        m.on_first_delta("s1"); // 无 inflight,no-op
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.gen_tokens_total, 5);
        assert_eq!(s.ttft_count, 0);
    }

    #[test]
    fn self_metrics_first_turn_tracked_per_session() {
        // 各 session 独立记录,不串台。
        let m = SelfMetrics::default();
        m.on_turn_started("a");
        m.on_turn_started("b");
        m.on_first_delta("a");
        m.on_first_delta("b");
        m.on_turn_complete("a", 1, 1, None, None);
        m.on_turn_complete("b", 1, 1, None, None);
        let s1 = m.snapshot();
        assert_eq!(s1.ttft_count, 2);
        assert_eq!(s1.gen_tokens_total, 2);
        // a 二轮继续记,b 仍只跑过首轮
        m.on_turn_started("a");
        m.on_first_delta("a");
        m.on_turn_complete("a", 1, 1, None, None);
        let s2 = m.snapshot();
        assert_eq!(s2.ttft_count, 3);
    }

    #[test]
    fn self_metrics_engine_fields_are_authoritative_for_ttft_tps() {
        // 引擎 TurnUsage 字段(含真实网络首字节)优先于墙钟重测。
        // 测试里墙钟近似 0,断言精确落在引擎值上即证明未走墙钟。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_message_delta("s1", 10); // 真实时序:首 delta 先于流末尾的 TurnUsage
        m.on_engine_usage("s1", Some(120), Some(1000), 50);
        m.on_turn_complete("s1", 100, 50, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!(
            (s.ttft_sum_s - 0.12).abs() < 1e-9,
            "ttft 应取引擎 first_token_ms=120ms,实际 {s:?}"
        );
        assert!(
            (s.tps_time_s - 0.88).abs() < 1e-9,
            "生成时长应取引擎 request_ms-first_token_ms=880ms,实际 {s:?}"
        );
        assert_eq!(s.tps_tokens, 50);
        assert_eq!(s.gen_tokens_total, 50);
    }

    #[test]
    fn self_metrics_engine_ttft_first_pair_later_pairs_accumulate() {
        // TurnUsage 每模型调用一条,无工具多调用轮(steer 续跑/reprompt 等)会有
        // 多条:TTFT 取首条完整对(turn 首个内容 token,后续不覆盖),生成窗口与
        // per-call tokens 逐条累加——TPS 是跨调用真实均速,分子分母同源。轮级
        // usage 与引擎分子独立存在时,优先取配对的引擎分子(180≠100)。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_engine_usage("s1", Some(120), Some(1000), 30);
        m.on_engine_usage("s1", Some(500), Some(4000), 70);
        m.on_turn_complete("s1", 10, 180, None, None);
        let s = m.snapshot();
        assert!((s.ttft_sum_s - 0.12).abs() < 1e-9, "实际 {s:?}");
        assert!(
            (s.tps_time_s - 4.38).abs() < 1e-9,
            "窗口应两条累加 0.88+3.5=4.38s,实际 {s:?}"
        );
        assert_eq!(
            s.tps_tokens, 100,
            "TPS 分子应取引擎逐条累加 30+70,实际 {s:?}"
        );
        assert_eq!(s.gen_tokens_total, 180);
    }

    #[test]
    fn self_metrics_engine_pair_never_mixes_across_calls() {
        // 半对(只有 ttft 或只有 request)不落值:两次调用各带一半,不得拼成完整对
        // (否则生成窗口 = A 事件的 request − B 事件的 ttft,跨调用失真,例如流
        // 中断重发时上一次尝试只留 request_ms 的场景)。此处直接驱动累加器钉逻辑。
        // >0 断言区分「回落墙钟」与「什么都没记」。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_message_delta("s1", 4);
        m.on_engine_usage("s1", Some(120), None, 5);
        m.on_engine_usage("s1", None, Some(4000), 5);
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!(
            s.ttft_sum_s > 0.0 && s.ttft_sum_s < 0.12,
            "两个半对不得拼成引擎对,应整组回落墙钟,实际 {s:?}"
        );
        assert!(
            s.tps_time_s > 0.0 && s.tps_time_s < 0.88,
            "应回落墙钟而非吃引擎派生时长,实际 {s:?}"
        );

        // 半对之后来的完整对整对生效:ttft、窗口、tokens 必须同来自第二条调用
        // (第二条 per-call tokens=7 ≠ 轮级 5,断言分子取配对引擎值而非轮级)。
        let m = SelfMetrics::default();
        m.on_turn_started("s2");
        m.on_engine_usage("s2", Some(120), None, 5);
        m.on_engine_usage("s2", Some(500), Some(4000), 7);
        m.on_turn_complete("s2", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!(
            (s.ttft_sum_s - 0.5).abs() < 1e-9 && (s.tps_time_s - 3.5).abs() < 1e-9,
            "应整对取自第二条调用(500,4000),不得拼成(120,4000),实际 {s:?}"
        );
        assert_eq!(
            s.tps_tokens, 7,
            "TPS 分子应取第二条调用的 per-call tokens,实际 {s:?}"
        );
    }

    #[test]
    fn self_metrics_partial_engine_fields_fall_back_to_wall_clock() {
        // 引擎只上报部分字段(无内容流/非流式 consult 路径 first_token_ms=None)
        // 时,TTFT 与生成时长整组回落墙钟,不混拼两套时钟;TPS 分子也随整组回落
        // 到轮级 usage。>0 断言区分「回落墙钟」与「什么都没记」。
        // 测试里墙钟远小于引擎值。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_message_delta("s1", 4);
        m.on_engine_usage("s1", Some(120), None, 7);
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!(
            s.ttft_sum_s > 0.0 && s.ttft_sum_s < 0.12,
            "应回落墙钟而非吃引擎 120ms,实际 {s:?}"
        );
        assert!(
            s.tps_time_s > 0.0 && s.tps_time_s < 0.88,
            "应回落墙钟而非吃引擎派生时长,实际 {s:?}"
        );
        assert_eq!(
            s.tps_tokens, 5,
            "引擎分子为 0(半对不落)应回落轮级 usage,实际 {s:?}"
        );
        assert_eq!(s.gen_tokens_total, 5);
    }

    #[test]
    fn self_metrics_engine_fields_ignored_on_tool_turn() {
        // D2 不变:带引擎字段的工具轮仍整体跳过 TTFT/TPS,tokens 照记。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_engine_usage("s1", Some(120), Some(1000), 50);
        m.on_tool("s1");
        m.on_turn_complete("s1", 200, 80, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 0);
        assert_eq!(s.tps_tokens, 0);
        assert_eq!(s.gen_tokens_total, 80);
    }

    #[test]
    fn self_metrics_engine_usage_without_start_is_noop() {
        // 中途接入(未收到 TurnStarted):无 inflight 不落值不 panic,
        // tokens 在收尾照记(与 delta 语义一致)。
        let m = SelfMetrics::default();
        m.on_engine_usage("s1", Some(120), Some(1000), 50);
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 0);
        assert_eq!(s.gen_tokens_total, 5);
    }

    #[test]
    fn self_metrics_zero_and_malformed_engine_windows_skip_tps() {
        // 零长度窗口(request_ms == first_token_ms):TTFT 照记(独立有效测量),
        // TPS 跳样——tokens 必须带非零时长,避免给聚合速度注水。畸形对
        // (request_ms < first_token_ms;引擎结构性不可达,防御性)饱和为
        // 零窗口,同样不 panic 不落 TPS。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_engine_usage("s1", Some(500), Some(500), 10);
        m.on_turn_complete("s1", 10, 10, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!((s.ttft_sum_s - 0.5).abs() < 1e-9, "实际 {s:?}");
        assert_eq!(s.tps_time_s, 0.0);
        assert_eq!(s.tps_tokens, 0);

        let m = SelfMetrics::default();
        m.on_turn_started("s2");
        m.on_engine_usage("s2", Some(1000), Some(120), 5);
        m.on_turn_complete("s2", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!((s.ttft_sum_s - 1.0).abs() < 1e-9, "实际 {s:?}");
        assert_eq!(s.tps_time_s, 0.0);
        assert_eq!(s.tps_tokens, 0);

        // 零窗口对 + 真实对混合的多调用轮:零窗口对两样都不落(tokens 必须带
        // 非零时长),TTFT 仍取零窗口首对;TPS 只来自真实对(1.5s/40)。
        // 若放宽「窗口 > 0 才累加」的门,分子会变成 50——此断言钉死该突变。
        let m = SelfMetrics::default();
        m.on_turn_started("s3");
        m.on_engine_usage("s3", Some(500), Some(500), 10);
        m.on_engine_usage("s3", Some(500), Some(2000), 40);
        m.on_turn_complete("s3", 10, 50, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!((s.ttft_sum_s - 0.5).abs() < 1e-9, "实际 {s:?}");
        assert!((s.tps_time_s - 1.5).abs() < 1e-9, "实际 {s:?}");
        assert_eq!(
            s.tps_tokens, 40,
            "零窗口对的 tokens 不得混入 TPS 分子,实际 {s:?}"
        );
    }

    #[test]
    fn self_metrics_engine_pairs_isolated_across_sessions() {
        // 引擎对按 session 隔离:a/b 交替投喂,各自取各自的对与 tokens,
        // 一方的轮级 usage 不得泄入另一方的 TPS。
        let m = SelfMetrics::default();
        m.on_turn_started("a");
        m.on_turn_started("b");
        m.on_engine_usage("a", Some(100), Some(1000), 20);
        m.on_engine_usage("b", Some(200), Some(2000), 40);
        m.on_turn_complete("a", 1, 99, None, None);
        m.on_turn_complete("b", 1, 98, None, None);
        let s = m.snapshot();
        // a: 窗口 0.9s/tokens 20;b: 窗口 1.8s/tokens 40。
        assert!(
            (s.tps_time_s - 2.7).abs() < 1e-9,
            "窗口应 a+b 独立累加 0.9+1.8=2.7s,实际 {s:?}"
        );
        assert_eq!(
            s.tps_tokens, 60,
            "分子应 a+b 各自 per-call tokens,实际 {s:?}"
        );
        assert_eq!(s.ttft_count, 2);
        assert!((s.ttft_sum_s - 0.3).abs() < 1e-9, "实际 {s:?}");
    }
}
