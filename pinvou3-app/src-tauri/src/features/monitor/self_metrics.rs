//! app 侧自测推理指标累加器。
//!
//! 职责边界——本模块只管「推理通路 TTFT / 生成速度 / 累计 tokens / KV 命中率」
//! 累加与读出（[`SelfMetrics`]），不涉及系统资源采集 (CPU/GPU/内存)与模型探针
//! （见 [`super::model_probe`]）。打点入口由 engine forwarder 在 TurnStarted /
//! 首个 MessageDelta / TurnUsage / ToolCallStarted / TurnComplete 五处调用;
//! 读出由 facade `sample_all` 经 `MonitorState` 触发。
//!
//! 计时源语义（审计 A1）：TTFT / TPS 优先采用引擎 `TurnUsage` 事件携带的
//! `first_token_ms` / `request_ms`——引擎在请求派发点起表,含真实网络首字节,
//! 是权威源;墙钟 `Instant` 重测发生在转发通路上,含事件分发延迟,仅作引擎未
//! 上报时的回落(旧引擎 / usage 未上报 / 非流式路径)。两组计时不同源不混拼。

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
    /// （工具轮墙钟含工具执行耗时，计进去会把速度拉低失真，D2 决定跳过）。
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
    /// 引擎 `TurnUsage` 上报的权威计时（审计 A1）：`first_token_ms` = 请求派发
    /// → 首 token（含真实网络首字节），`request_ms` = 请求派发 → 流结束。
    /// 收尾时优先于墙钟重测；`None` 表示引擎本轮未上报（回落墙钟）。
    engine_ttft_ms: Option<u64>,
    engine_request_ms: Option<u64>,
}

/// app 侧自测推理指标累加器。流式转发通路（engine.rs forwarder）在
/// TurnStarted / 首个 MessageDelta / ToolCallStarted / TurnComplete 四处打点写入，
/// `sample_all` 读出。`inflight` 按 `session_id` 键控——多 session 并发各测各的，
/// 不串台；`perf` 是全局单调累计，各轮把自己的增量加进去。
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
                engine_request_ms: None,
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

    /// 引擎 `TurnUsage` 事件：记录本轮模型调用的权威计时。TurnUsage 每次模型
    /// 调用发一条——纯文本轮恰一条；工具轮首条也先于 ToolCallStarted 到达，
    /// 取首条上报值即可（工具轮收尾时整体跳过 TTFT/TPS，D2 不变）。字段缺失
    /// （`None`，旧引擎 / 非流式路径）不落值，收尾整组回落墙钟。
    pub fn on_engine_usage(
        &self,
        session_id: &str,
        first_token_ms: Option<u64>,
        request_ms: Option<u64>,
    ) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            if t.engine_ttft_ms.is_none() {
                t.engine_ttft_ms = first_token_ms;
            }
            if t.engine_request_ms.is_none() {
                t.engine_request_ms = request_ms;
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
                // 计时源优先级（审计 A1）：引擎 TurnUsage 字段是权威源
                // (first_token_ms 从请求派发点起表,含真实网络首字节;request_ms
                // 同钟收束于流结束,两者相减即纯生成窗口)。引擎整组字段缺失时
                // (旧引擎 / usage 未上报 / 非流式路径)回落墙钟 Instant 重测;
                // TTFT 与生成时长必须同源,不混拼两套时钟。
                let (ttft_s, gen_s) = match (t.engine_ttft_ms, t.engine_request_ms) {
                    (Some(ttft_ms), Some(request_ms)) => {
                        let ttft = ttft_ms as f64 / 1000.0;
                        let gen_window_s = request_ms.saturating_sub(ttft_ms) as f64 / 1000.0;
                        (Some(ttft), Some(gen_window_s))
                    }
                    _ => match t.first {
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
                    let tps_units = if output_tokens > 0 {
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
        // 审计 A1:引擎 TurnUsage 字段(含真实网络首字节)优先于墙钟重测。
        // 测试里墙钟近似 0,断言精确落在引擎值上即证明未走墙钟。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_message_delta("s1", 10); // 真实时序:首 delta 先于流末尾的 TurnUsage
        m.on_engine_usage("s1", Some(120), Some(1000));
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
    fn self_metrics_first_engine_usage_wins() {
        // TurnUsage 每模型调用一条,首条上报值生效,后续调用不覆盖
        // (纯文本轮只有一条;多调用属工具轮,收尾被 D2 跳过)。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_engine_usage("s1", Some(120), Some(1000));
        m.on_engine_usage("s1", Some(500), Some(4000));
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert!((s.ttft_sum_s - 0.12).abs() < 1e-9, "实际 {s:?}");
        assert!((s.tps_time_s - 0.88).abs() < 1e-9, "实际 {s:?}");
    }

    #[test]
    fn self_metrics_partial_engine_fields_fall_back_to_wall_clock() {
        // 引擎只上报部分字段(非流式/重试路径 first_token_ms=None)时,TTFT 与
        // 生成时长整组回落墙钟,不混拼两套时钟。测试里墙钟远小于引擎值。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_message_delta("s1", 4);
        m.on_engine_usage("s1", Some(120), None);
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 1);
        assert!(s.ttft_sum_s < 0.12, "应回落墙钟而非吃引擎 120ms,实际 {s:?}");
        assert!(
            s.tps_time_s < 0.88,
            "应回落墙钟而非吃引擎派生时长,实际 {s:?}"
        );
        assert_eq!(s.gen_tokens_total, 5);
    }

    #[test]
    fn self_metrics_engine_fields_ignored_on_tool_turn() {
        // D2 不变:带引擎字段的工具轮仍整体跳过 TTFT/TPS,tokens 照记。
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_engine_usage("s1", Some(120), Some(1000));
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
        m.on_engine_usage("s1", Some(120), Some(1000));
        m.on_turn_complete("s1", 10, 5, None, None);
        let s = m.snapshot();
        assert_eq!(s.ttft_count, 0);
        assert_eq!(s.gen_tokens_total, 5);
    }
}
