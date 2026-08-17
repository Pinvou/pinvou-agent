//! macOS 系统 Speech 框架后端（Phase 2）。
//!
//! 替代打包的 SenseVoice ASR 引擎：用 `SFSpeechURLRecognitionRequest` 对**文件**
//! 做语音识别（前端 Web Audio ScriptProcessorNode + JS encodeWav 已产出 16kHz
//! mono 16-bit PCM WAV，正是 Speech framework 的首选格式 → 零转码、零 ffmpeg）。
//!
//! 线程模型：[`transcribe_with_speech`] 内部用 mpsc channel 把 Obj-C 异步结果
//! handler 同步化，前端 `invoke` 本就是 async，Tauri runtime 不会因此阻塞 UI 线程。
//! `task` / `request` / `recognizer` 三个 `Retained` 强引用在函数作用域内保活，
//! 直到 channel 收到 final 结果或超时——过早 drop `SFSpeechRecognitionTask` 会取消
//! 识别（见 Apple 文档 SFSpeechRecognizer/recognitionTask(with:resultHandler:)）。
//!
//! On-device vs. Apple 服务识别在运行时按当前 recognizer/locale 的
//! `supportsOnDeviceRecognition` 决定：支持时强制端上识别，否则由系统使用在线
//! Speech 服务。该能力不是处理器架构的固定属性。
//!
//! 参考的 objc2 调用风格：`pet_window.rs:1048-1076`；block2 回调包装见 crate 文档
//! （`block2::RcBlock::new`，`DynBlock` 是 `Block` 的类型别名）。

#![cfg(target_os = "macos")]

use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;

use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSError, NSLocale, NSString, NSURL};
use objc2_speech::{
    SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognitionTaskHint,
    SFSpeechRecognizer, SFSpeechURLRecognitionRequest,
};

/// 根据 on-device 支持情况决定识别模式（纯函数，可单测）。
///
/// `supports_on_device` 来自 `SFSpeechRecognizer::supportsOnDeviceRecognition()`：
/// - true → 要求端上识别。
/// - false → 不强制端上，由系统 Speech 服务在线处理。
///
/// 目前是直传（语义：能端上就端上），但抽成函数是为后续加策略（如用户偏好
/// "强制联网换更高精度"）留扩展点，也便于单测覆盖。
pub fn decide_on_device(supports_on_device: bool) -> bool {
    // 支持端上识别时明确要求端上；不支持时由系统走在线 Speech 服务。
    supports_on_device
}

/// 对 wav 文件做语音识别，返回识别文本。
///
/// 前端录音 → 16kHz mono 16-bit PCM WAV（首选格式，零转码）→ 写到临时文件 →
/// 传文件 URL 给 `SFSpeechURLRecognitionRequest`。
///
/// `locale_tag` 决定识别语言（如 `zh-CN` / `en-US` / `ja-JP`，来自 UI 语言偏好
/// [`crate::platform::prefs::Language::speech_recognition_locale`]）。**不可用系统默认
/// locale**：macOS 系统语言为英文时，默认 locale = en-US → 中文语音被当英文解析，
/// 产出无意义英文字母。显式 `initWithLocale` 锁定与 UI 一致的语言。
///
/// 同步等待识别结果（Obj-C 异步 + mpsc channel），60s 超时（Apple 对单次识别有
/// ~1 分钟硬限）。
pub fn transcribe_with_speech(wav_path: &Path, locale_tag: &str) -> Result<String, String> {
    // 0. 确保已授权。这是首次语音输入能跑通的前提：SFSpeechRecognizer 默认
    //    NotDetermined，只有 requestAuthorization 才弹系统授权框。不调则永不弹框，
    //    isAvailable 仍可能为 true → 进 recognitionTask → error 203 或 60s 超时。
    //    放在 URL 构造前，未授权时尽早失败、不浪费后续对象创建。
    ensure_authorized()?;

    // 1. 构造 file:// URL。wav_path 必须是有效 UTF-8 路径（跨平台临时目录默认是）。
    let path_str = wav_path
        .to_str()
        .ok_or_else(|| "wav 路径含非 UTF-8 字符".to_string())?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));

    // 2. recognizer（按 locale_tag 锁定识别语言，而非系统默认 locale）。
    //    用 initWithLocale 创建：系统语言为英文时默认 locale=en-US，会把中文音频当
    //    英文解析 → 无意义英文字母。显式 zh-CN/en-US/ja-JP 与 UI 语言一致。
    // SAFETY: localeWithLocaleIdentifier 是 safe 类方法（返回 Retained，自动释放）；
    // initWithLocale 的 alloc 配对由 Retained::Drop 释放。locale_tag 来自可信常量映射。
    let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str(locale_tag));
    let recognizer: Retained<SFSpeechRecognizer> =
        unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale) }
            .ok_or_else(|| {
                format!("系统不支持该语音识别 locale（{locale_tag}），请检查语言设置")
            })?;
    // SAFETY: isAvailable 只读属性，无外部输入。
    if !unsafe { recognizer.isAvailable() } {
        return Err("系统语音识别服务不可用（未授权 / 联网失败 / 服务限流）".to_string());
    }

    // 3. on-device vs. Apple 服务：按 recognizer/locale 的运行时能力判断，
    //    不能按 Apple Silicon / Intel 架构预判。
    //    requiresOnDeviceRecognition 仅在 supportsOnDeviceRecognition=true 时设置。
    // SAFETY: supportsOnDeviceRecognition 只读属性。
    let on_device = decide_on_device(unsafe { recognizer.supportsOnDeviceRecognition() });

    // 4. 文件识别请求。SFSpeechURLRecognitionRequest 是 SFSpeechRecognitionRequest 的
    //    子类，initWithURL: 接受文件 URL（不是 Data）。
    // SAFETY: alloc/init 配对；URL 由上一步构造的有效 NSString 路径生成。
    let request: Retained<SFSpeechURLRecognitionRequest> = unsafe {
        SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &url)
    };
    // SAFETY: 两个 setter 只配置 request 对象状态，无外部副作用。
    unsafe {
        request.setRequiresOnDeviceRecognition(on_device);
        // Dictation hint：与键盘听写同类任务，倾向于产出可读文本（带标点、ITN）。
        request.setTaskHint(SFSpeechRecognitionTaskHint::Dictation);
    }

    // 5. 同步等待识别结果：mpsc channel + block2::RcBlock 包装 Obj-C completion。
    //    result handler 可能被多次调用（partial → final），只在 isFinal 时取
    //    bestTranscription.formattedString；error 时立即报错。
    let (tx, rx) = mpsc::channel();
    // RcBlock 把闭包拷到堆（_NSConcreteMallocBlock），由 RcBlock 的 Drop 释放。
    // recognitionTaskWithRequest_resultHandler 会 Block_copy 这个 block（task 结束前
    // 不释放），所以闭包只要在 task 真正调用它之前不被释放即可——这里 block 活到函数末尾。
    let block = block2::RcBlock::new(
        move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
            // error 非 nil 时优先报错（即使 result 也非 nil）。
            if let Some(err) = NonNull::new(error) {
                // SAFETY: NonNull::new 保证非空；框架保证生命周期横跨此次回调。
                let err = unsafe { err.as_ref() };
                // localizedDescription 是 objc2-foundation 中标为 safe 的方法（只读不可变属性）。
                let desc = err.localizedDescription().to_string();
                let _ = tx.send(Err(format!("语音识别错误: {desc}")));
                return;
            }
            if let Some(res) = NonNull::new(result) {
                // SAFETY: 同上。
                let res = unsafe { res.as_ref() };
                // SAFETY: isFinal / bestTranscription / formattedString 都是只读访问。
                if unsafe { res.isFinal() } {
                    let text = unsafe { res.bestTranscription().formattedString() }.to_string();
                    let _ = tx.send(Ok(text));
                }
                // 非 final（partial）结果：忽略，等最终结果。
            }
        },
    );

    // SAFETY: recognizer 已校验 isAvailable=true；request 已正确初始化；
    // block 的签名与 recognitionTaskWithRequest:resultHandler: 的
    // `void (^)(SFSpeechRecognitionResult *_Nullable, NSError *_Nullable)` 匹配。
    // objc2-speech 内部会 Block_copy 保留 block，task 期间不会被释放。
    let task: Retained<SFSpeechRecognitionTask> =
        unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &block) };

    // 6. 阻塞等待 final 结果或超时。task / request / recognizer 三个 Retained 在本
    //    作用域结尾统一 drop；超时情况下 drop task 会取消识别（符合预期）。
    let result = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .map_err(|_| "语音识别超时（60s，可能音频过长或服务限流）".to_string())?;

    // 显式 drop task（语义：识别已完成，task 可释放；超时路径这里跳过无妨）。
    // request / recognizer / block 紧随函数作用域自然 drop。
    drop(task);
    drop(request);
    drop(recognizer);
    drop(block);
    result
}

/// 把授权状态映射为「能否识别」的纯函数（可单测，无需真机）。
///
/// `Authorized` → Ok；其余三个状态 → Err，文案引导用户自助修复。
/// `NotDetermined` 正常路径下不会出现在这里（[`ensure_authorized`] 已先 resolve），
/// 但保留分支以防 `ensure_authorized` 之外有路径直接读 `authorizationStatus`。
pub fn auth_status_decision(
    status: objc2_speech::SFSpeechRecognizerAuthorizationStatus,
) -> Result<(), String> {
    use objc2_speech::SFSpeechRecognizerAuthorizationStatus as S;
    match status {
        S::Authorized => Ok(()),
        S::NotDetermined => Err(
            "语音识别尚未授权（未决定）。请再次触发语音输入以弹出授权请求".to_string(),
        ),
        S::Denied => Err(
            "语音识别权限已被拒绝。请到「系统设置 > 隐私与安全性 > 语音识别」开启 pinvou3 的权限后重试"
                .to_string(),
        ),
        S::Restricted => Err("该设备限制使用语音识别（受 MDM / 家长控制等策略管控）".to_string()),
        // NSInteger 的 #[non_exhaustive] 防御：未来苹果新增状态时不编译失败。
        // 注：status 为 objc 枚举仅实现 Debug 未实现 Display，故保留 {:?}（与
        // {:#} 的统一约定不冲突——该分支不是错误值透传，而是未知状态诊断）。
        _ => Err(format!("语音识别授权状态异常: {status:?}")),
    }
}

/// 确保已获语音识别授权；未决定时触发系统授权弹框（幂等）。
///
/// **这是首次语音输入能跑通的前提**：`SFSpeechRecognizer` 默认授权状态是
/// `NotDetermined`，只有显式调用 [`SFSpeechRecognizer::requestAuthorization`] 才会
/// 弹出系统授权框（`NSSpeechRecognitionUsageDescription` 提供 prompt 文案）。
/// 不调用 → 永不弹框 → `isAvailable` 仍可能为 true → 进 recognitionTask → 框架
/// 返回 error 203（permission denied）或静默不回调 → 60s 超时。
///
/// 幂等：已决定（Authorized/Denied/Restricted）时 handler 立即以当前状态回调，
/// 不会重复弹框，故每次 transcribe 调用一次安全。
///
/// handler 「不保证在 main queue 回调」（Apple 文档），故用 mpsc 同步跨线程。
fn ensure_authorized() -> Result<(), String> {
    use objc2_speech::SFSpeechRecognizer;
    // SAFETY: authorizationStatus 是只读类方法，无副作用，任何线程可调。
    let current = unsafe { SFSpeechRecognizer::authorizationStatus() };
    // 已授权直接放行；已拒绝/受限立即报错（不重复弹框）。
    if let Err(e) = auth_status_decision(current) {
        // NotDetermined 才往下走弹框流程；其余状态直接返回错误。
        if current != objc2_speech::SFSpeechRecognizerAuthorizationStatus::NotDetermined {
            return Err(e);
        }
    } else {
        return Ok(());
    }

    // 触发系统授权弹框并同步等待用户响应。
    let (tx, rx) = mpsc::channel();
    let block = block2::RcBlock::new(
        move |status: objc2_speech::SFSpeechRecognizerAuthorizationStatus| {
            let _ = tx.send(status);
        },
    );
    // SAFETY: handler 签名 `Fn(SFSpeechRecognizerAuthorizationStatus)` 与
    // requestAuthorization: 的 block 匹配；RcBlock 保活到 send 完成。
    unsafe { SFSpeechRecognizer::requestAuthorization(&block) };
    let status = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .map_err(|_| "语音识别授权请求超时（60s 未收到用户响应）".to_string())?;
    auth_status_decision(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_decision_ok_when_authorized() {
        assert!(auth_status_decision(
            objc2_speech::SFSpeechRecognizerAuthorizationStatus::Authorized
        )
        .is_ok());
    }

    #[test]
    fn auth_decision_err_when_not_determined() {
        let err = auth_status_decision(
            objc2_speech::SFSpeechRecognizerAuthorizationStatus::NotDetermined,
        )
        .unwrap_err();
        assert!(err.contains("尚未授权"), "unexpected: {err}");
    }

    #[test]
    fn auth_decision_err_when_denied_guides_to_settings() {
        let err = auth_status_decision(objc2_speech::SFSpeechRecognizerAuthorizationStatus::Denied)
            .unwrap_err();
        // 文案必须引导用户去系统设置（否则用户无从自救）。
        assert!(err.contains("系统设置"), "unexpected: {err}");
        assert!(err.contains("语音识别"), "unexpected: {err}");
    }

    #[test]
    fn auth_decision_err_when_restricted() {
        assert!(auth_status_decision(
            objc2_speech::SFSpeechRecognizerAuthorizationStatus::Restricted
        )
        .is_err());
    }
}
