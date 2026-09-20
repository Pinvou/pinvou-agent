//! CPU 采样的纯节拍数学 —— linux/macos 采样器共享。
//!
//! FFI(`host_statistics64`)与 `/proc` 读取留在各平台文件(`platform/{macos,linux}_cpu.rs`);
//! 这里只收与平台无关的部分:聚合节拍结构、两次采样的差分→使用率换算、
//! 百分比钳制,以及跨两次调用的差分状态机。Windows 的 `SystemTimes` 口径不同
//! (100ns 粒度、busy 由 total−idle 反推),不共用本模块。

/// `/proc/stat` 首行 / `host_statistics64` 的聚合节拍。busy 含 user/nice/system/
/// irq/softirq/steal(steal 时间本机不可用,计入占用而非空闲);idle 含 iowait
/// (等 IO 视为空闲)。注意内核文档明言 iowait 本身口径不可靠(无任务可执行且有
/// 未完成 IO 才计入,等待中退出的任务会回退为 idle);异常时本采样最多导致
/// total_delta 为 0 返回 None,不会产生失真数值。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SystemTicks {
    pub busy: u64,
    pub idle: u64,
}

/// 两次采样的差分状态机(进程级单例由各平台文件持 Mutex 保护):
/// `absorb` 收下当前采样,返回"上一采样 → 当前采样"的系统使用率;
/// 首次调用无上一采样,返回 None。
#[derive(Debug, Default)]
pub(crate) struct SystemDeltaState {
    system: Option<SystemTicks>,
}

impl SystemDeltaState {
    pub(crate) fn absorb(&mut self, current: Option<SystemTicks>) -> Option<f64> {
        let usage_pct = match (self.system, current) {
            (Some(prev), Some(current)) => system_usage_pct(prev, current),
            _ => None,
        };
        if current.is_some() {
            self.system = current;
        }
        usage_pct
    }
}

/// 两次采样间的系统使用率百分比。计数器回绕(busy 变小)时 `checked_sub`
/// 返回 None,不产生负值。
pub(crate) fn system_usage_pct(prev: SystemTicks, current: SystemTicks) -> Option<f64> {
    let busy_delta = current.busy.checked_sub(prev.busy)?;
    let idle_delta = current.idle.saturating_sub(prev.idle);
    let total_delta = busy_delta.checked_add(idle_delta)?;
    if total_delta == 0 {
        return None;
    }
    Some(clamp_pct(busy_delta as f64 * 100.0 / total_delta as f64))
}

/// 使用率钳到 0-100;非有限值(NaN 等)按 0 处理。
pub(crate) fn clamp_pct(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_usage_pct_clamps_range() {
        assert_eq!(clamp_pct(-1.0), 0.0);
        assert_eq!(clamp_pct(42.5), 42.5);
        assert_eq!(clamp_pct(120.0), 100.0);
        assert_eq!(clamp_pct(f64::NAN), 0.0);
    }

    #[test]
    fn system_usage_from_deltas_returns_expected() {
        let prev = SystemTicks {
            busy: 100,
            idle: 400,
        };
        let current = SystemTicks {
            busy: 200,
            idle: 500,
        };
        // delta busy 100 / total 200 = 50%
        assert_eq!(system_usage_pct(prev, current), Some(50.0));
    }

    #[test]
    fn system_usage_returns_none_without_elapsed_ticks() {
        let sample = SystemTicks {
            busy: 100,
            idle: 400,
        };
        assert_eq!(system_usage_pct(sample, sample), None);
    }

    #[test]
    fn system_usage_clamps_wraparound() {
        // 计数器回绕(busy 变小)时 checked_sub 返回 None,不产生负值。
        let prev = SystemTicks {
            busy: 500,
            idle: 100,
        };
        let current = SystemTicks {
            busy: 100,
            idle: 600,
        };
        assert_eq!(system_usage_pct(prev, current), None);
    }

    #[test]
    fn delta_state_skips_first_sample_and_keeps_latest() {
        let mut state = SystemDeltaState::default();
        // 首次采样无上一采样 → None,但采样本身被吸收
        assert_eq!(
            state.absorb(Some(SystemTicks {
                busy: 100,
                idle: 400
            })),
            None
        );
        // 第二次采样产出使用率
        assert_eq!(
            state.absorb(Some(SystemTicks {
                busy: 200,
                idle: 500
            })),
            Some(50.0)
        );
        // 采样失败(None)不更新状态:沿用上一采样,返回 None
        assert_eq!(state.absorb(None), None);
        assert_eq!(
            state.absorb(Some(SystemTicks {
                busy: 300,
                idle: 600
            })),
            Some(50.0)
        );
    }
}
