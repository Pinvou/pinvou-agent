//! Opt-in prompt evaluation through Pinvou's product turn lifecycle.
//!
//! Run with an isolated PINVOU3_HOME and a JSON AgenticTaskRequest file.
//! `code_mode` and `workspace_roots` supply the same bridge inputs as the GUI;
//! the request, model selection, tools, submission and collection remain owned
//! by the existing product runtime. This host does not create browser windows.

use std::path::PathBuf;
use std::sync::Arc;

use pinvou3_lib::agentic_task::{AgenticTaskRequest, run_agentic_task};
use pinvou3_lib::headless_bridge::run_windowless_host;

fn evaluation_options(
    value: &serde_json::Value,
) -> Result<(bool, Vec<PathBuf>), serde_json::Error> {
    let code_mode = value
        .get("code_mode")
        .map(|mode| serde_json::from_value(mode.clone()))
        .transpose()?
        .unwrap_or(false);
    let roots = value
        .get("workspace_roots")
        .map(|roots| serde_json::from_value(roots.clone()))
        .transpose()?
        .unwrap_or_default();
    Ok((code_mode, roots))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PINVOU3_HOME").is_none_or(|home| home.is_empty()) {
        return Err("set an isolated PINVOU3_HOME before running live evaluations".into());
    }
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: prompt_eval <request.json>; set an isolated PINVOU3_HOME")?;
    let value: serde_json::Value = serde_json::from_reader(std::fs::File::open(path)?)?;
    let (code_mode, roots) = evaluation_options(&value)?;
    let request: AgenticTaskRequest = serde_json::from_value(value)?;
    let report = run_windowless_host(move |mut pool, store| async move {
        let predicate: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |_| code_mode);
        pool.bridge.set_code_session_predicate(predicate.clone());
        store.set_code_session_predicate(predicate);
        pool.bridge
            .set_workspace_roots_resolver(Arc::new(move |_| roots.clone()));
        run_agentic_task(pool, store, request).await
    })?;
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::evaluation_options;
    use serde_json::json;

    #[test]
    fn options_keep_defaults_and_explicit_code_workspace() {
        let (code_mode, roots) = evaluation_options(&json!({})).unwrap();
        assert!(!code_mode);
        assert!(roots.is_empty());
        let (code_mode, roots) = evaluation_options(&json!({
            "code_mode": true, "workspace_roots": ["project"]
        }))
        .unwrap();
        assert!(code_mode);
        assert_eq!(roots, vec![std::path::PathBuf::from("project")]);
    }

    #[test]
    fn options_reject_malformed_mode_and_roots() {
        for value in [
            json!({"code_mode": "false"}),
            json!({"code_mode": null}),
            json!({"workspace_roots": "project"}),
            json!({"workspace_roots": [42]}),
        ] {
            assert!(evaluation_options(&value).is_err());
        }
    }
}
