use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceShortcutKey {
    Alt,
    Space,
    Escape,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceShortcutEvent {
    TriggerDictation,
}

/// 全局 Alt 语音快捷键的手势状态(仅 Windows 低层钩子写入)。
///
/// Alt 按下即吞(tap-hold):若之后出现组合键,平台层补发一次合成 Alt down
/// (`inject_alt_down`)恢复系统/WebView 的 Alt+组合键行为;若空按,则在抬起时
/// 吞掉 up 并触发听写。WebView 要幺看到完整的 down/up 对,要幺两者都看不到,
/// 不会残留"Alt 仍按住"的修饰键状态。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct VoiceShortcutState {
    alt_down: bool,
    alt_pending: bool,
    /// 已为组合键按 [Alt↓, 组合键↓] 保序重放(平台层在 SendInput 成功后置位),
    /// 真实 Alt up 必须放行与之配对。
    alt_forwarded: bool,
    /// 本次手势内 Space down 已被吞;只有配对吞掉 up,不误吞 Alt 按下前
    /// 就已按下的 Space 的 up(否则系统级 VK_SPACE 卡在按下态)。
    space_swallowed: bool,
    /// Alt 按下时的前台窗口句柄(0 = 未知);抬起时比对,防止按住 Alt 切窗后
    /// 在新窗口松手误触。
    alt_hwnd: isize,
    /// 上一个事件的 KBDLLHOOKSTRUCT.time(毫秒 tick;0 = 尚无事件),供陈旧
    /// 手势兜底(UAC/锁屏丢 keyup 后状态卡死)。
    last_event_ms: u32,
}

/// 事件间隔超过该阈值即视为上一手势已死(UAC/锁屏吞掉了 keyup),整体复位。
/// 正常按住时 OS 自动重复会持续刷新事件流,不会触发;系统禁用自动重复的
/// 无障碍配置下,超过 2s 的超长按住不再触发(可接受的兜底代价)。
const STALE_GESTURE_MS: u32 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VoiceShortcutDecision {
    event: Option<VoiceShortcutEvent>,
    suppress: bool,
    /// 当前组合键 down 已被吞:平台层须用单次 SendInput 按 [Alt↓, 组合键↓]
    /// 保序重放(成功后确认 alt_forwarded),否则系统会因 Alt down 被吞而收不到
    /// 该组合键(Alt+Tab / Alt+F4 失效)。
    inject_alt_down: bool,
}

impl VoiceShortcutDecision {
    const fn pass() -> Self {
        Self {
            event: None,
            suppress: false,
            inject_alt_down: false,
        }
    }

    const fn suppress(event: Option<VoiceShortcutEvent>) -> Self {
        Self {
            event,
            suppress: true,
            inject_alt_down: false,
        }
    }

    /// 吞掉当前组合键 down,交平台层保序重放。
    const fn forward_combo() -> Self {
        Self {
            event: None,
            suppress: true,
            inject_alt_down: true,
        }
    }
}

/// `active` = 本次击键归快捷键手势接管(开关开启且前台是本进程目标窗口)。
/// `foreground_hwnd` 为当前前台窗口句柄(0 = 无前台窗口)。
/// `time_ms` 为 KBDLLHOOKSTRUCT.time(毫秒 tick,允许回绕)。
fn handle_voice_shortcut_key(
    state: &mut VoiceShortcutState,
    key: VoiceShortcutKey,
    key_down: bool,
    active: bool,
    foreground_hwnd: isize,
    time_ms: u32,
) -> VoiceShortcutDecision {
    // 陈旧手势兜底:UAC/锁屏等场景 keyup 丢失会让 alt_down 卡住,下一个手势
    // 的首个事件会被误判为长按重复或幽灵触发。事件间隔超过阈值即整体复位,
    // 再按新事件正常处理(tick 回绕用 wrapping_sub)。
    if state.last_event_ms != 0 && time_ms.wrapping_sub(state.last_event_ms) > STALE_GESTURE_MS {
        *state = VoiceShortcutState::default();
    }
    state.last_event_ms = time_ms;

    if !active {
        // 手势落到非目标窗口:不吞键,仅在 Alt 抬起时复位,避免状态泄漏到下一次手势。
        if key == VoiceShortcutKey::Alt && !key_down {
            *state = VoiceShortcutState::default();
        }
        return VoiceShortcutDecision::pass();
    }

    match (key, key_down) {
        (VoiceShortcutKey::Alt, true) => {
            if state.alt_down {
                // 长按自动重复:未转发的继续吞,已转发的放行(与合成 down 保持一致)。
                return if state.alt_forwarded {
                    VoiceShortcutDecision::pass()
                } else {
                    VoiceShortcutDecision::suppress(None)
                };
            }
            state.alt_down = true;
            state.alt_pending = true;
            state.alt_forwarded = false;
            state.alt_hwnd = foreground_hwnd;
            VoiceShortcutDecision::suppress(None)
        }
        (VoiceShortcutKey::Alt, false) => {
            if !state.alt_down {
                return VoiceShortcutDecision::pass();
            }
            let hwnd_mismatch =
                state.alt_hwnd != 0 && foreground_hwnd != 0 && state.alt_hwnd != foreground_hwnd;
            let trigger = state.alt_pending && !hwnd_mismatch;
            let forwarded = state.alt_forwarded;
            *state = VoiceShortcutState::default();
            if forwarded {
                // 组合键路径:真实 up 放行,与合成 down 配对;pending 已被组合键清除,不会触发。
                VoiceShortcutDecision::pass()
            } else {
                // 空按(或组合键仅 Space):down 已吞,up 成对吞掉并触发。
                VoiceShortcutDecision::suppress(
                    trigger.then_some(VoiceShortcutEvent::TriggerDictation),
                )
            }
        }
        (VoiceShortcutKey::Space, true) if state.alt_down => {
            // Alt+Space 会弹窗口系统菜单:成对吞掉 down/up,且不为它补发 Alt down。
            state.alt_pending = false;
            state.space_swallowed = true;
            VoiceShortcutDecision::suppress(None)
        }
        // 只配对吞「down 已被吞」的 Space up;Alt 按下前就已按下的 Space,
        // 它的 down 早已放行,up 必须同样放行,否则系统级卡键。
        (VoiceShortcutKey::Space, false) if state.alt_down && state.space_swallowed => {
            state.alt_pending = false;
            VoiceShortcutDecision::suppress(None)
        }
        // Alt+Esc(系统窗口循环切换)与普通组合键同口径:不重放会以裸 Esc 漏进
        // WebView,且 pending 残留会在 Alt up 时误触发听写。
        (VoiceShortcutKey::Other | VoiceShortcutKey::Escape, true) if state.alt_down => {
            if state.alt_pending {
                state.alt_pending = false;
                if !state.alt_forwarded {
                    // alt_forwarded 由平台层在 [Alt↓, 组合键↓] 保序重放成功后确认;
                    // 重放失败保持未转发,真实 Alt up 按未转发路径吞掉收尾,不留残态。
                    return VoiceShortcutDecision::forward_combo();
                }
            }
            VoiceShortcutDecision::pass()
        }
        _ => VoiceShortcutDecision::pass(),
    }
}

#[derive(Clone, Serialize)]
struct VoiceShortcutTriggerPayload {
    mode: &'static str,
    source: &'static str,
    /// 目标窗口 label(挂载了 VoiceShortcutRouter 的窗口),前端校验与本窗一致才消费。
    window_label: String,
    /// 路由依据:"recording"(定向录音窗,用于停止/互斥)或 "focused"(聚焦窗,
    /// 正常触发)。前端据此刻别「以录音窗身份被路由但自身已无活跃会话」的陈旧
    /// 登记(WebView 重载后 JS 会话重建而原生登记未清),清除后丢弃,不再后台幽灵开麦。
    route: &'static str,
}

/// 当前录音中的窗口 label(前端在录音开始/结束时通过命令同步)。
/// 用于跨窗录音互斥:A 窗录音中,B 窗的 Alt 手势定向到 A 窗(停止),绝不双开。
static RECORDING_LABEL: Mutex<Option<String>> = Mutex::new(None);

/// 由 `set_voice_shortcut_recording` 命令调用;前端在录音开始/结束/出错时带上本窗口 label 同步。
pub(crate) fn set_recording_label(label: Option<String>) {
    if let Ok(mut guard) = RECORDING_LABEL.lock() {
        *guard = label.filter(|value| !value.trim().is_empty());
    }
}

pub(crate) fn recording_label() -> Option<String> {
    RECORDING_LABEL.lock().ok().and_then(|guard| guard.clone())
}

fn clear_recording_label() {
    if let Ok(mut guard) = RECORDING_LABEL.lock() {
        *guard = None;
    }
}

/// 窗口销毁时主动解除登记:录音中的窗口被直接关闭时,前端来不及走
/// finishVoiceInput 收口;若不清掉,原生钩子会把 Alt 手势一直路由进已销毁
/// 窗口(emit 对已销毁窗口不报错,失败兜底不会触发,等效全局吞键黑洞)。
pub(crate) fn forget_recording_window(label: &str) {
    if recording_label().as_deref() == Some(label) {
        clear_recording_label();
    }
}

/// 只有主窗与撕离窗(DetachedShell)挂载 VoiceShortcutRouter、能消费快捷键事件;
/// 桌宠(pet)、code-reader、artifact 等窗口不在白名单:不吞键、不 emit。
fn is_voice_shortcut_router_window(label: &str) -> bool {
    label == "main" || label.starts_with("detached-")
}

/// Alt 空按触发的路由:录音中的窗口优先(定向停止、跨窗互斥),
/// 否则定向到聚焦的白名单窗口;两者都没有则返回 None(不吞键、不 emit)。
/// 返回值附带路由依据,供前端区分陈旧的录音窗登记(见 payload.route)。
fn resolve_trigger_target(
    recording_label: Option<&str>,
    focused_router_label: Option<&str>,
) -> Option<(String, &'static str)> {
    if let Some(label) = recording_label {
        return Some((label.to_string(), "recording"));
    }
    focused_router_label.map(|label| (label.to_string(), "focused"))
}

mod platform;

pub(crate) fn install(app: AppHandle) {
    platform::install(app);
}

pub(crate) fn set_enabled(enabled: bool) {
    platform::set_enabled(enabled);
}

/// 定向 emit:只发到目标窗口,无聚焦/无目标窗口时静默丢弃(不再全窗广播)。
fn emit_shortcut_event(
    app: &AppHandle,
    event: VoiceShortcutEvent,
    window_label: &str,
    route: &'static str,
) {
    match event {
        VoiceShortcutEvent::TriggerDictation => {
            let result = app.emit_to(
                window_label,
                "voice-shortcut:trigger",
                VoiceShortcutTriggerPayload {
                    mode: "dictation",
                    source: "native",
                    window_label: window_label.to_string(),
                    route,
                },
            );
            match result {
                Ok(()) => {
                    log::debug!(
                        "voice shortcut emitted event=TriggerDictation window={}",
                        window_label
                    );
                }
                Err(error) => {
                    log::warn!(
                        "voice shortcut emit failed event=TriggerDictation window={} error={}",
                        window_label,
                        error
                    );
                    // 目标窗口已销毁:若它还被记为录音窗,清掉陈旧 label,避免后续手势被黑洞。
                    if recording_label().as_deref() == Some(window_label) {
                        clear_recording_label();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HWND_A: isize = 100;
    const HWND_B: isize = 200;

    #[test]
    fn alt_tap_swallows_down_and_up_symmetrically_and_triggers() {
        let mut state = VoiceShortcutState::default();
        let down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        assert_eq!(down.event, None);
        assert!(down.suppress);
        assert!(!down.inject_alt_down);

        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn alt_autorepeat_stays_swallowed_and_triggers_once() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        let repeat =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        assert_eq!(repeat, VoiceShortcutDecision::suppress(None));

        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));

        let stray =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(stray, VoiceShortcutDecision::pass());
    }

    #[test]
    fn alt_combo_injects_alt_down_once_and_forwards_real_up() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);

        let combo =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo, VoiceShortcutDecision::forward_combo());
        assert!(combo.suppress);

        // 平台层 SendInput 成功后确认 alt_forwarded(重放 [Alt↓, 组合键↓] 完成)。
        state.alt_forwarded = true;

        // 同一组合期间第二个按键不再补发。
        let combo2 =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo2, VoiceShortcutDecision::pass());

        // 组合键后 Alt 自动重复 down 放行,与合成 down 一致。
        let repeat =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        assert_eq!(repeat, VoiceShortcutDecision::pass());

        // 真实 up 放行配对,不触发听写。
        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(up, VoiceShortcutDecision::pass());
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn alt_escape_combo_forwards_alt_down_once_and_never_triggers() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);

        // Alt+Esc 与普通组合键同口径:吞掉并保序重放 [Alt↓, Esc↓],Esc 不裸漏。
        let escape =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Escape, true, true, HWND_A, 0);
        assert_eq!(escape, VoiceShortcutDecision::forward_combo());
        state.alt_forwarded = true;

        // 真实 Alt up 放行并与合成 down 配对,不触发听写。
        let alt_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(alt_up, VoiceShortcutDecision::pass());
        assert!(!alt_up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn combo_replay_failure_leaves_no_residue() {
        // SendInput 失败:alt_forwarded 未被平台层确认,组合键已丢,真实 Alt up
        // 按未转发路径吞掉收尾,不触发、状态复位,不留"Alt 仍按住"残态。
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        let combo =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo, VoiceShortcutDecision::forward_combo());
        // (不模拟平台层确认)

        let combo_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, false, true, HWND_A, 0);
        assert_eq!(combo_up, VoiceShortcutDecision::pass());

        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(up, VoiceShortcutDecision::suppress(None));
        assert_eq!(up.event, None);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn stale_gesture_resets_after_lost_keyup() {
        // UAC/锁屏吞掉 keyup:下一事件与上一事件间隔远超阈值,整体复位。
        // 此后到达的 Alt up 不再幽灵触发听写,随后的新手势完整可用。
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 1000);

        let ghost_up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt,
            false,
            true,
            HWND_A,
            90000,
        );
        assert_eq!(ghost_up, VoiceShortcutDecision::pass());
        // 手势态已复位(last_event_ms 保留当前 tick 供后续间隔判定)。
        assert!(!state.alt_down);
        assert!(!state.alt_pending);
        assert!(!state.alt_forwarded);

        // 新手势:down 吞、up 触发,行为与首次完全一致。
        let down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 90500);
        assert!(down.suppress);
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt,
            false,
            true,
            HWND_A,
            90620,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn alt_repeat_within_gesture_does_not_trip_stale_reset() {
        // 正常按住:OS 自动重复持续刷新事件流,间隔远小于阈值,不触发复位。
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 1000);
        let repeat =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 1600);
        assert_eq!(repeat, VoiceShortcutDecision::suppress(None));
        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 1630);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn alt_space_suppresses_pair_and_never_triggers_or_injects() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);

        let space_down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(space_down, VoiceShortcutDecision::suppress(None));

        let space_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, false, true, HWND_A, 0);
        assert_eq!(space_up, VoiceShortcutDecision::suppress(None));

        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(up, VoiceShortcutDecision::suppress(None));
        assert_eq!(up.event, None);
    }

    #[test]
    fn space_up_pressed_before_alt_is_not_swallowed() {
        // Space 在 Alt 之前按下:down 已放行(钩子只见 Alt+Space 才吞),
        // 期间按住 Alt 再松 Space,up 必须放行,否则系统级 VK_SPACE 卡在按下态。
        let mut state = VoiceShortcutState::default();
        let space_down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(space_down, VoiceShortcutDecision::pass());

        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);

        let space_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, false, true, HWND_A, 0);
        assert_eq!(space_up, VoiceShortcutDecision::pass());

        // 同一手势内随后真正按下的 Alt+Space 仍成对吞掉。
        let pair_down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(pair_down, VoiceShortcutDecision::suppress(None));
        let pair_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, false, true, HWND_A, 0);
        assert_eq!(pair_up, VoiceShortcutDecision::suppress(None));
    }

    #[test]
    fn space_then_alt_uses_plain_alt_only() {
        let mut state = VoiceShortcutState::default();
        let space =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(space, VoiceShortcutDecision::pass());

        let alt =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        assert_eq!(alt, VoiceShortcutDecision::suppress(None));

        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_A, 0);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
    }

    #[test]
    fn alt_up_in_another_app_window_does_not_trigger() {
        let mut state = VoiceShortcutState::default();
        let down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        assert!(down.suppress);

        // 按住 Alt 切到同进程另一窗口后松手:不触发;down 已吞,up 成对吞掉。
        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, HWND_B, 0);
        assert_eq!(up.event, None);
        assert!(up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn unknown_foreground_hwnd_still_allows_trigger() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, 0, 0);
        let up = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true, 0, 0);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn shortcuts_are_ignored_when_no_target_window() {
        let mut state = VoiceShortcutState::default();
        let down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, false, HWND_A, 0);
        assert_eq!(down, VoiceShortcutDecision::pass());
        assert_eq!(state, VoiceShortcutState::default());

        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, false, HWND_A, 0);
        assert_eq!(up, VoiceShortcutDecision::pass());
    }

    #[test]
    fn gesture_state_resets_when_focus_leaves_target_mid_hold() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true, HWND_A, 0);
        // 按住 Alt 焦点离开目标窗口:期间按键全部放行,Alt 抬起时复位状态。
        let other =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, false, HWND_B, 0);
        assert_eq!(other, VoiceShortcutDecision::pass());
        let up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, false, HWND_B, 0);
        assert_eq!(up, VoiceShortcutDecision::pass());
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn escape_is_not_emitted_without_frontend_state() {
        let mut state = VoiceShortcutState::default();
        let escape =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Escape, true, true, HWND_A, 0);
        assert_eq!(escape, VoiceShortcutDecision::pass());
    }

    #[test]
    fn router_window_whitelist_only_covers_main_and_detached() {
        assert!(is_voice_shortcut_router_window("main"));
        assert!(is_voice_shortcut_router_window(
            "detached-session-0123456789abcdef"
        ));
        assert!(is_voice_shortcut_router_window(
            "detached-persona-fedcba9876543210"
        ));
        assert!(!is_voice_shortcut_router_window("pet"));
        assert!(!is_voice_shortcut_router_window("code-reader"));
        assert!(!is_voice_shortcut_router_window(
            "artifact-0123456789abcdef"
        ));
        assert!(!is_voice_shortcut_router_window(""));
    }

    #[test]
    fn recording_window_wins_over_focused_window() {
        assert_eq!(
            resolve_trigger_target(Some("main"), Some("detached-session-0123456789abcdef")),
            Some(("main".to_string(), "recording"))
        );
    }

    #[test]
    fn without_recording_routes_to_focused_router_window() {
        assert_eq!(
            resolve_trigger_target(None, Some("detached-session-0123456789abcdef")),
            Some(("detached-session-0123456789abcdef".to_string(), "focused"))
        );
    }

    #[test]
    fn no_recording_and_no_focused_router_window_means_no_emit() {
        assert_eq!(resolve_trigger_target(None, None), None);
    }
}
