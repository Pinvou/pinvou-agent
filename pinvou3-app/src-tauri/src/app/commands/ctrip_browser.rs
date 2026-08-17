use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;
use tauri::{Manager, Url, WebviewUrl, WebviewWindowBuilder};

const CTRIP_ASSIST_LABEL: &str = "ctrip-assist";

fn validate_ctrip_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url).map_err(|error| format!("invalid ctrip url: {error}"))?;
    if parsed.scheme() != "https" {
        return Err("ctrip assist only allows https urls".to_string());
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed =
        host == "www.ctrip.com" || host.ends_with(".ctrip.com") || host == "flights.ctrip.com";
    if !allowed {
        return Err("ctrip assist only allows ctrip.com pages".to_string());
    }
    Ok(parsed)
}

#[tauri::command]
pub async fn open_ctrip_assist_window(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url = validate_ctrip_url(&url)?;
    if let Some(existing) = app.get_webview_window(CTRIP_ASSIST_LABEL) {
        existing
            .navigate(url)
            .map_err(|error| format!("navigate ctrip assist window: {error}"))?;
        let _ = existing.set_focus();
        return Ok(());
    }

    WebviewWindowBuilder::new(&app, CTRIP_ASSIST_LABEL, WebviewUrl::External(url))
        .title("携程协助")
        .inner_size(1280.0, 820.0)
        .center()
        .resizable(true)
        .build()
        .map_err(|error| format!("build ctrip assist window: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn run_ctrip_search_assist(
    app: tauri::AppHandle,
    details: Value,
) -> Result<Value, String> {
    let Some(window) = app.get_webview_window(CTRIP_ASSIST_LABEL) else {
        return Err("ctrip assist window is not open".to_string());
    };

    let details_json =
        serde_json::to_string(&details).map_err(|error| format!("serialize details: {error}"))?;
    let script = build_search_script(&details_json);
    let (sender, receiver) = mpsc::channel::<String>();
    window
        .eval_with_callback(script, move |value| {
            let _ = sender.send(value);
        })
        .map_err(|error| format!("inject ctrip assist script: {error}"))?;

    let raw = receiver
        .recv_timeout(Duration::from_secs(6))
        .map_err(|_| "ctrip assist script did not return a result".to_string())?;
    serde_json::from_str::<Value>(&raw).or_else(|_| {
        Ok(json!({
            "ok": true,
            "message": raw,
        }))
    })
}

fn build_search_script(details_json: &str) -> String {
    format!(
        r#"
(async () => {{
  const details = {details_json};
  const sleep = (ms) => new Promise(resolve => setTimeout(resolve, ms));
  const textOf = (node) => (node && (node.innerText || node.textContent || node.value || '') || '').trim();
  const normalize = (value) => String(value || '').trim();
  const pageText = () => normalize(document.body && document.body.innerText);
  const result = {{
    ok: false,
    blocked: false,
    filled: [],
    clickedSearch: false,
    reason: '',
    url: location.href
  }};

  if (/验证码|安全验证|滑块|扫码登录|请登录|登录后|风控|实名/.test(pageText())) {{
    result.blocked = true;
    result.reason = '页面需要登录、验证码、安全验证或实名信息，已暂停自动操作。';
    return result;
  }}

  const fire = (element) => {{
    element.dispatchEvent(new Event('input', {{ bubbles: true }}));
    element.dispatchEvent(new Event('change', {{ bubbles: true }}));
    element.dispatchEvent(new KeyboardEvent('keyup', {{ bubbles: true, key: 'Enter' }}));
  }};
  const editable = Array.from(document.querySelectorAll('input, textarea, [contenteditable="true"]'))
    .filter(element => !element.disabled && element.offsetParent !== null);
  const labelOf = (element) => [
    element.getAttribute('placeholder'),
    element.getAttribute('aria-label'),
    element.getAttribute('title'),
    element.getAttribute('name'),
    element.id,
    textOf(element.closest('label')),
    textOf(element.parentElement),
  ].filter(Boolean).join(' ');
  const findInput = (patterns) => editable.find(element => patterns.some(pattern => pattern.test(labelOf(element))));
  const setInput = async (element, value, label) => {{
    value = normalize(value);
    if (!element || !value) return false;
    element.focus();
    if (element.isContentEditable) element.textContent = value;
    else element.value = value;
    fire(element);
    await sleep(180);
    result.filled.push(label);
    return true;
  }};

  await setInput(findInput([/出发|起点|from/i]), details.origin, 'origin');
  await setInput(findInput([/到达|目的地|终点|to/i]), details.destination, 'destination');
  await setInput(findInput([/日期|出发日|depart|date/i]), details.date, 'date');
  await setInput(findInput([/成人|adult/i]), details.adults, 'adults');
  await setInput(findInput([/儿童|child/i]), details.children, 'children');

  const clickByText = (patterns) => {{
    const candidates = Array.from(document.querySelectorAll('button, a, [role="button"], .btn, .search-btn, .searchBtn'))
      .filter(element => element.offsetParent !== null);
    const target = candidates.find(element => patterns.some(pattern => pattern.test(textOf(element) || labelOf(element))));
    if (!target) return false;
    target.click();
    return true;
  }};

  result.clickedSearch = clickByText([/搜索|查询|查找|Search/i]);
  result.ok = result.filled.length > 0 || result.clickedSearch;
  if (!result.ok) result.reason = '未识别到可自动填写的携程搜索控件。';
  result.url = location.href;
  return result;
}})();
"#
    )
}
