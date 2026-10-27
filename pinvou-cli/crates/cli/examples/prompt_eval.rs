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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("PINVOU3_HOME").is_none() {
        return Err("set an isolated PINVOU3_HOME before running live evaluations".into());
    }
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: prompt_eval <request.json>; set an isolated PINVOU3_HOME")?;
    let value: serde_json::Value = serde_json::from_reader(std::fs::File::open(path)?)?;
    let code_mode = value
        .get("code_mode")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let roots: Vec<PathBuf> = value
        .get("workspace_roots")
        .map(|roots| serde_json::from_value(roots.clone()))
        .transpose()?
        .unwrap_or_default();
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
