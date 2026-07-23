//! 全功能 e2e：真实 SQLite + 真实扫描 + 真实 file_ingest 解析 + 真实 bge-m3 embedding。
//! 不 mock，跑真实组件。L2(RAG) 需 vLLM + Pinvou3Bridge，另见 rag 单测。
//!
//! 跑法(需先下 bge-m3 到 ~/models/bge-m3)：
//!   cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml --lib \
//!     knowledge::e2e_test --ignored --nocapture -- --test-threads=1

#![cfg(test)]

use std::fs;
use std::time::{Duration, Instant};

use super::store::SearchQuery;
use super::KnowledgeService;

fn wait_idle(svc: &KnowledgeService, what: &str) {
    let start = Instant::now();
    loop {
        let s = svc.status();
        if !s.running {
            return;
        }
        if start.elapsed() > Duration::from_secs(60) {
            panic!("{what} 超时未完成: phase={}", s.phase);
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

#[test]
fn service_starts_without_embedder() {
    let root = std::env::temp_dir().join(format!(
        "pinvou3_kb_startup_without_embedder_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();

    let svc = KnowledgeService::new(&root.join("index.db")).expect("KnowledgeService::new");
    assert!(
        !svc.semantic_ready(),
        "同步服务初始化不得加载 embedding 模型"
    );

    drop(svc);
    let _ = fs::remove_dir_all(&root);
}

#[test]
#[ignore]
fn full_l0_l1_e2e() {
    // L1 语义检索真实跑：指向本地 bge-m3。
    let home = std::env::var("HOME").unwrap();
    std::env::set_var(
        "PINVOU3_KB_EMBED_MODEL_DIR",
        format!("{home}/models/bge-m3"),
    );

    let root = std::env::temp_dir().join(format!("pinvou3_e2e_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("docs")).unwrap();
    // L0 多类型 + 一对内容相同的重复文件
    fs::write(
        root.join("docs/季度报告.md"),
        "# 季度销售报告\n华东区增长 20%，华南区持平。重点客户续约率 85%。",
    )
    .unwrap();
    fs::write(
        root.join("docs/访谈纪要.md"),
        "# 用户访谈\n受访者认为保险报价流程过于繁琐，希望一键比价。竞品在交强险环节体验更顺畅。",
    )
    .unwrap();
    fs::write(
        root.join("docs/副本.md"),
        "# 季度销售报告\n华东区增长 20%，华南区持平。重点客户续约率 85%。",
    )
    .unwrap();
    fs::write(root.join("notes.txt"), "随手记：周五前提交预算。").unwrap();
    fs::write(root.join("archive.zip"), b"PK\x03\x04 fake zip").unwrap();
    // 白名单外：源码 + 编译产物，应被类型白名单排除、不入库
    fs::write(root.join("main.c"), "int main(){return 0;}").unwrap();
    fs::write(root.join("App.class"), b"\xca\xfe\xba\xbe").unwrap();

    let db = root.join("index.db");
    let svc = KnowledgeService::new(&db).expect("KnowledgeService::new");
    assert!(svc.reload_embedder(), "测试 embedding 模型应能后台热加载");

    // ───── L0：扫描 ─────
    svc.start_scan(vec![root.clone()]);
    wait_idle(&svc, "scan");
    let stats = svc.store.stats().expect("stats");
    assert!(
        stats.total_files >= 5,
        "应扫到 ≥5 常用文件，实际 {}",
        stats.total_files
    );

    // ───── L0：类型白名单(源码/编译产物不入库) ─────
    let src = svc
        .store
        .search(&SearchQuery {
            text: Some("main".into()),
            limit: 50,
            ..Default::default()
        })
        .expect("search src");
    assert!(
        !src.iter().any(|h| h.name == "main.c"),
        "源码 main.c 应被白名单排除"
    );
    let cls = svc
        .store
        .search(&SearchQuery {
            text: Some("App".into()),
            limit: 50,
            ..Default::default()
        })
        .expect("search class");
    assert!(
        !cls.iter().any(|h| h.name == "App.class"),
        "编译产物 App.class 应被白名单排除"
    );

    // ───── L0：按名搜索 ─────
    let by_name = svc
        .store
        .search(&SearchQuery {
            text: Some("报告".into()),
            limit: 50,
            ..Default::default()
        })
        .expect("search name");
    assert!(
        by_name.iter().any(|h| h.name.contains("季度报告")),
        "应搜到 季度报告.md"
    );

    // ───── L0：按类型过滤 ─────
    let by_ext = svc
        .store
        .search(&SearchQuery {
            exts: vec!["md".into()],
            limit: 50,
            ..Default::default()
        })
        .expect("search ext");
    assert!(by_ext.len() >= 3, "应有 ≥3 个 .md，实际 {}", by_ext.len());
    assert!(
        by_ext.iter().all(|h| h.ext.as_deref() == Some("md")),
        "过滤后应全为 md"
    );

    // ───── L0：类型计数 ─────
    let tc = svc.store.type_counts().expect("type_counts");
    let md = tc
        .iter()
        .find(|t| t.ext == "md")
        .map(|t| t.count)
        .unwrap_or(0);
    assert!(md >= 3, "md 计数应 ≥3，实际 {}", md);

    // ───── L1：知识集 + 真实解析 + embedding ─────
    assert!(
        svc.l1().has_embedder(),
        "L1 embedding 应已启用(env 配了 bge-m3)"
    );
    let cid = svc
        .l1()
        .create_collection("调研集", Some("调研"), Some("访谈与报告"))
        .expect("create coll");
    assert_eq!(
        svc.l1().ingest_file(cid, &root.join("docs/访谈纪要.md")),
        "parsed"
    );
    assert_eq!(
        svc.l1().ingest_file(cid, &root.join("docs/季度报告.md")),
        "parsed"
    );

    // ───── L1：关键词检索(命中原词) ─────
    let kw = svc
        .l1()
        .retrieve_for_chat(cid, "交强险", 5, 0)
        .expect("kw search");
    assert!(!kw.is_empty(), "应检索到含'交强险'的块");
    assert!(
        kw.iter().any(|h| h.text.contains("交强险")),
        "命中块应含'交强险'"
    );
    assert!(kw[0].doc_name.contains("访谈"), "溯源应指向访谈纪要");

    // ───── L1：语义检索(同义不同词，考验向量) ─────
    let sem = svc
        .l1()
        .retrieve_for_chat(cid, "车险价格怎么比较", 5, 0)
        .expect("semantic search");
    assert!(!sem.is_empty(), "语义查询应有召回(向量路径)");
    assert!(
        sem.iter().any(|h| h.text.contains("报价")
            || h.text.contains("比价")
            || h.text.contains("交强险")),
        "语义召回应命中保险报价相关块，实际 top: {}",
        sem.first().map(|h| h.text.as_str()).unwrap_or("")
    );

    // ───── L1：文档列表 + 计数 ─────
    let docs = svc.l1().list_documents(cid, 0).expect("docs");
    assert_eq!(docs.len(), 2, "应有 2 个文档");
    assert!(docs.iter().all(|d| d.parse_status == "parsed"));
    let coll = svc
        .l1()
        .list_collections()
        .expect("colls")
        .into_iter()
        .find(|c| c.id == cid)
        .unwrap();
    assert_eq!(coll.doc_count, 2);
    assert!(coll.chunk_count >= 2, "应有 ≥2 块");

    // ───── L1：删文档级联 ─────
    svc.l1().remove_document(docs[0].id).expect("remove doc");
    let after = svc.l1().list_documents(cid, 0).expect("docs2");
    assert_eq!(after.len(), 1, "删后应剩 1 文档");

    svc.cancel_scan();
    let _ = fs::remove_dir_all(&root);
    eprintln!(
        "✅ L0+L1 e2e PASS — 扫描 {} 文件 / 知识集 {} 块 / 关键词+语义检索均命中并溯源",
        stats.total_files, coll.chunk_count
    );
}
