//! app 侧自测推理指标累加器。
//!
//! 职责边界——本模块只管「流式转发通路上就地测的 TTFT / 生成速度 / 累计
//! tokens / KV 命中率」累加与读出（[`SelfMetrics`]），不涉及系统资源采集
//! (CPU/GPU/内存)与模型探针（见 [`super::model_probe`]）。打点入口由
//! engine forwarder 在 TurnStarted / 首个 MessageDelta / ToolCallStarted /
//! TurnComplete 四处调用;读出由 facade `sample_all` 经 `MonitorState` 触发。

use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, Default, Serialize)]
pub struct SelfMetricsDebugSnapshot {
    pub inflight_count: usize,
    pub warmed_sessions_count: usize,
    pub last_event: Option<String>,
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
}

/// app 侧自测推理指标累加器。流式转发通路（engine.rs forwarder）在
/// TurnStarted / 首个 MessageDelta / ToolCallStarted / TurnComplete 四处打点写入，
/// `sample_all` 读出。`inflight` 按 `session_id` 键控——多 session 并发各测各的，
/// 不串台；`perf` 是全局单调累计，各轮把自己的增量加进去。
#[derive(Debug, Default)]
pub struct SelfMetrics {
    perf: Mutex<SelfPerfInner>,
    inflight: Mutex<HashMap<String, TurnTiming>>,
    /// 已完成过至少一轮的 session。每 session 首个完成轮 = 带底座 cache warmup 的**冷轮**
    /// (warmup 同步跑完整段冷 prefill,TurnStarted→首token 窗口吃满冷启),TTFT/TPS 不代表
    /// 稳态,故跳过(tokens 照记)。warmup 恰好只在 session 首轮跑,此集合精确识别那一轮。
    warmed_sessions: Mutex<HashSet<String>>,
    last_event: Mutex<Option<String>>,
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
            },
        );
        self.set_last_event(format!("turn_started session={session_id}"));
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
                self.set_last_event(format!("first_delta session={session_id}"));
            }
            t.output_chars = t.output_chars.saturating_add(char_count as u64);
        }
    }

    /// 本轮出现过工具调用 → 标记。收尾时据此跳过 TTFT/TPS（D2）。
    pub fn on_tool(&self, session_id: &str) {
        if let Some(t) = self.inflight.lock().get_mut(session_id) {
            t.had_tool = true;
            self.set_last_event(format!("tool session={session_id}"));
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
        let is_first_turn = self.warmed_sessions.lock().insert(session_id.to_string());
        let mut p = self.perf.lock();
        p.gen_tokens_total += output_tokens as u64;
        p.prompt_tokens_total += input_tokens as u64;
        if let Some(h) = cache_hit {
            p.cache_hit_tokens += h as u64;
        }
        if let Some(m) = cache_miss {
            p.cache_miss_tokens += m as u64;
        }
        let mut recorded_perf = false;
        let had_timing = timing.is_some();
        if let Some(t) = timing {
            if !t.had_tool {
                if let Some(first) = t.first {
                    p.ttft_sum_s += first.duration_since(t.start).as_secs_f64();
                    p.ttft_count += 1;
                    let gen_s = first.elapsed().as_secs_f64();
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
                        recorded_perf = true;
                    }
                }
            }
        }
        self.set_last_event(format!(
            "turn_complete session={session_id} input={input_tokens} output={output_tokens} first_turn={is_first_turn} had_timing={had_timing} recorded_perf={recorded_perf}"
        ));
    }

    /// Turn aborted（停止/会话回收，未走到 TurnComplete）：移除 inflight 打点条目，
    /// 避免该 session 的 TurnTiming 永久驻留。不写 perf 累计、不标 warmed（中断轮非完成轮）。
    pub fn on_turn_aborted(&self, session_id: &str) {
        self.inflight.lock().remove(session_id);
        self.set_last_event(format!("turn_aborted session={session_id}"));
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

    pub fn debug_snapshot(&self) -> SelfMetricsDebugSnapshot {
        SelfMetricsDebugSnapshot {
            inflight_count: self.inflight.lock().len(),
            warmed_sessions_count: self.warmed_sessions.lock().len(),
            last_event: self.last_event.lock().clone(),
        }
    }

    fn set_last_event(&self, value: String) {
        *self.last_event.lock() = Some(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_metrics_aborted_turn_clears_inflight_without_marking_warmed() {
        let m = SelfMetrics::default();
        m.on_turn_started("s1");
        m.on_first_delta("s1");
        m.on_turn_aborted("s1");
        let dbg = m.debug_snapshot();
        assert_eq!(dbg.inflight_count, 0);
        assert_eq!(dbg.warmed_sessions_count, 0);
        // 中断轮不污染 perf 累计：不写 tokens、不计 TTFT/TPS。
        let s = m.snapshot();
        assert_eq!(s.gen_tokens_total, 0);
        assert_eq!(s.prompt_tokens_total, 0);
        assert_eq!(s.ttft_count, 0);
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
}
