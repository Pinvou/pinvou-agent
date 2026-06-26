# DeepSeek-TUI Fork 淇敼娓呭崟

> pinvou3 瀵?`DeepSeek-TUI`(宸?rebrand `CodeWhale`)搴曞骇鎵€鏈?fork 淇敼鐨?*鍗曚竴鐜扮姸娓呭崟**銆?
> 鐢ㄩ€?鈶?sync 鍚庢煡 patch 瀛樻椿 鈶?浜ゆ帴 / onboarding 鈶?涓婃父 PR 瀹氫綅鏀瑰姩鐐广€?
> 閰嶅:`scripts/fork-guard.sh`(鎸囩汗 + 鍥炲綊娴嬭瘯瀹堝崼)銆乣docs/fork-policy.md`(缁存姢绛栫暐 + sync 娴佺▼ + PR 鐘舵€?銆?
>
> **褰撳墠鍩虹嚎**:submodule 鍒嗘敮 `pinvou3-clean` 鈫?upstream **v0.8.60**;HEAD `1161bc78` = v0.8.60 + 8 涓婚 commit(2026-06-15 clean re-fork,绾挎€у巻鍙?銆?

---

## 0. 褰撳墠鐘舵€侀€熻(2026-06-15)

| 椤?| 鍊?|
|---|---|
| submodule 鍒嗘敮 | **`pinvou3-clean`**(`.gitmodules` 杩借釜);HEAD `1161bc78`;澶囦唤 `backup/pre-reclean-v0.8.60` |
| fork drift | **+2335 / 鈭?60 琛?43 鏂囦欢**(`git -C DeepSeek-TUI diff v0.8.60..HEAD --shortstat`)銆傝秴 1500 杞笂闄?涓讳綋鏄伐浣滄祦灞?W鈥斺€斿睘"鎺ュ彈閲?fork"(fork-policy 搂0);app 灞?prompt 璧?override 娉ㄥ叆,涓嶈鍏?|
| 鍘嗗彶 | v0.8.60 + 8 commit:C1 lib 路 C2 blocklist 路 C3 append_file 路 C4 safety 路 C5+C7 prompt-composer 路 C6 chore 路 W 宸ヤ綔娴佸眰 路 docs銆傚悗缁彔鍔?C8 浼氳瘽宸ュ叿寮€鍏?op(#4,`a0efea0b`,2026-06-23) 路 R extra_tools 娉ㄥ叆鍙?`6b3059da`,2026-06-24) |
| LLM 鏆撮湶 native 宸ュ叿 | **23 涓?*(鍏ㄩ噺娉ㄥ唽 鈭?81 榛戝悕鍗?**tool_search 宸茬鐢?*,妯″瀷鏃犳硶婵€娲?deferred 宸ュ叿)銆侻CP `mcp_pinvou_present_artifact` 鍙︽帴,鍏?24 鍏ュ彛 |
| fork-guard | **49 鎸囩汗 + 鍥炲綊娴嬭瘯**(`scripts/fork-guard.sh`;+C8 浼氳瘽宸ュ叿寮€鍏?2 鏉?+RAG1/RAG2 瀹?extra_tools 娉ㄥ叆鍙?;搴曞骇 lib 4539 pass(+1 宸茬煡 flake:verifier 鍚庡彴 shell 骞惰璇姤)/ app lib 195 pass(鍗曠嚎绋? |
| system prompt | dump 閫愬瓧鑺傜ǔ瀹?210 琛?diff=0);per-turn `<runtime_prompt>` tag + goal continuation 鍧囧凡 gate |

---

## 1. fork 缁撴瀯(C1鈥揅8 + R + W 閫昏緫涓婚)

> 閫昏緫鍒嗙粍,瀵瑰簲涓婚 commit銆傜湅鏌愭枃浠?fork-distinct 鏀瑰姩:`git -C DeepSeek-TUI diff v0.8.60..HEAD -- <file>`銆?
> 鍐茬獊鏄撳嚭琛€浼樺厛绾?sync review 椤哄簭):**prompts.rs(C5+C7) > turn_loop.rs(C7) > subagent/mod.rs(W) > tool_catalog.rs(C2) > project_context.rs(C5)**銆?

### C1 `lib` library facade
- **鏂囦欢**:`crates/tui/src/lib.rs`(鏁存枃浠垛€斺€斾笂娓稿彧鏈?`main.rs`,鏃?lib target)
- **鏀瑰姩**:`pub mod` 鏆撮湶鍐呴儴妯″潡 + `#[cfg(test)] pub mod test_support`,璁?pinvou3-app 浠?`deepseek_tui::*` as-library 璋冪敤 + `cargo test --lib` 鑳借窇
- **鈿狅笍 缁存姢**:涓婃父姣忓姞/鍒犳ā鍧楄**鎵嬪姩鍚屾 `pub mod`**(涓婃父鏃?lib.rs,3-way 涓嶄細鑷姩鏀?銆傚鍎?`pub mod` 浼氱紪璇戦敊(v0.8.51 `cycle_manager` / v0.8.60 `prompt_persist` 鍒犻櫎鍗虫鍧?;`acp_server` 渚濊禆 bin 涓撳睘绗﹀彿涓嶈兘杩?lib
- 涓婃父 PR:鉂?pinvou3 涓撶敤

### C2 `tools` blocklist 宸ュ叿闂ㄦ帶
- **鏂囦欢**:`tools/pinvou3_blocklist.rs`(鏂板缓,**81 鏉￠粦鍚嶅崟**)銆乣core/engine/tool_catalog.rs`銆乣tools/registry.rs`銆乣tools/mod.rs`
- **鍝插**:涓婃父(v0.8.47 璧?鏄?**allowlist**;pinvou3 鐩稿弽鈥斺€?*鏄剧ず鍏ㄩ儴銆佸彧闅愯棌榛戝悕鍗?*,缁?Qwen3.6 绮剧畝鍒?**23 宸ュ叿**
- **鍏抽敭**:`pinvou3_should_defer_native_tool(name, mode, always_load)` **mode-aware**:Yolo 鍙?defer 榛戝悕鍗曘€俙request_user_input` 璺ㄦ墍鏈?mode 纭繚鐣?鍚﹀垯 GUI 涓嶅嚭閫夋嫨姘旀场);`image_analyze` 鏀惧嚭(闇€ bridge 寮€ `VisionModel` feature);`checklist_*` 鏈夋剰鍙銆俙PINVOU3_BLOCKLIST_OVERRIDE` env 渚?L1 harness 瑙ｉ攣
- **鈿狅笍 tool_search 闃插尽**:blocklist 鏄€宒efer 涓嶅垹闄ゃ€?宸ュ叿浠嶅湪 catalog銆備笂娓?`tool_search`(`ensure_advanced_tooling` 娉ㄥ叆)鑳借妯″瀷**鎼滅储婵€娲昏 blocklist 鐨?deferred 宸ュ叿**鈫?鍑荤┛闂ㄦ帶銆備慨娉?`tool_search_*` 杩?blocklist + **娉ㄥ叆澶?gate**(`is_pinvou3_hidden(TOOL_SEARCH_*)` 涓虹湡涓嶆敞鍏?鈫?catalog 鏍规湰涓嶅惈
- **娴嬭瘯**:`pinvou3_yolo_offers_nonblocklisted_tools_outside_upstream_default`銆乣forkguard_tool_search_not_injected_*`
- 涓婃父 PR:鉂?鍝插鐩稿弽

### C3 `tools` append_file + 澶т骇鐗╀繚鎶?
- **鏂囦欢**:`tools/file.rs`銆乣core/engine/dispatch.rs`銆乣client/chat.rs`銆乣tools/registry.rs`(`with_file_tools`)銆乣tui/approval.rs`銆乣tui/widgets/tool_card.rs`銆乣tools/approval_cache.rs`
- **鏀瑰姩**:`append_file` 宸ュ叿(涓婃父娌℃湁)+ content **64KB 纭笂闄?* + `truncated_args_hint`(娴佹埅鏂己瀛楁鈫掑紩瀵煎垎鍧?+ SSE idle-timeout 閬ユ祴 + undo 蹇収绾冲叆
- **鐞嗙敱**:鏈湴鎱?vLLM 澶т骇鐗?PPT/闀挎枃妗?>240s idle timeout 娴佹埅鏂?`write_file` 鍐?skeleton(鈮?KB)鈫?`append_file` 杩藉姞 chunk(鈮?6KB)
- **娴嬭瘯**:`truncated_args_hint_*`銆乣test_{write,append}_file_rejects_oversized_content`
- 涓婃父 PR:鉂?涓庝笓灞?`append_file` 娣辫€﹀悎,鍘昏€﹀悗鏃犺惤鐐?

### C4 `safety` careful 瀹夊叏 hook
- **鏂囦欢**:`tools/shell.rs`銆乣command_safety.rs`
- **鏀瑰姩**:Dangerous 鍛戒护(`rm -rf /`路`~`路`$HOME`路`/*`銆乫ork bomb)鍦?**YOLO 涔?BLOCKED**(涓婃父 YOLO 璺宠繃)銆?YOLO 鍙槸鍏嶅鎵瑰脊绐?涓嶇瓑浜庡厑璁告瘉鐏€у懡浠?
- **涓轰綍鐣?*(2026-06-15 璇勪及):pinvou3 榛樿 YOLO + 寮辨ā鍨?+ workspace=$HOME,杩欐槸鍞竴鎷?`rm -rf ~` 鐨勭綉(deny hook 鍙鐩栨晱鎰熻矾寰?sudo,upstream careful 鍦?YOLO 璺宠繃)銆傚彲绉?app deny hook 浣嗕細鍓婂急(瑁稿瓧绗︿覆鍖归厤 vs `analyze_command` 瑙ｅ寘瑁?+ 涓?`safety_level` 绾㈠崱 metadata),涓嶅€煎緱
- **娴嬭瘯**:`forkguard` careful shell YOLO-block 鎸囩汗 + `command_safety` Dangerous 娴嬭瘯(鍚?`bash -lc 'rm -rf /'` 鍖呰９)
- 涓婃父 PR:鉂?瀹夊叏妯″瀷涓撶敤 路(C4-a 澶氳閫愯宸茶涓婃父 `split_command_segments` harvest)

### C5 `prompt` GUI prompt / context / skills
- **鏂囦欢**:`project_context.rs`銆乣project_context_cache.rs`銆乣skills/mod.rs`銆乣commands/groups/skills/skills.rs`銆乣tools/skill.rs`銆乣prompts.rs`(涓?C7 鍏辨鏂囦欢)
- **project_context**:`PROJECT_CONTEXT_FILES`/`GLOBAL_PATHS` **鐮嶇┖**(workspace=$HOME GUI 鍔╂墜,涓嶈鍏朵粬 AI 宸ュ叿閰嶇疆);`load_repo_constitution_block` **鐭矾**;`generate_ephemeral_context` **鐮嶇┖杩?None**(闃?$HOME 鏍戞壂鎴?overview 娉ㄥ叆 prompt,浠呴噰涓婃父鍑芥暟鍚嶈璋冪敤鐐圭紪璇?
- **skills**:鎵弿璺緞鍙暀 `~/.agents/skills`(鍘?10 璺緞,#41;union 鎺ョ嚎宸茶涓婃父 harvest,鍙墿璺緞鏀剁獎)
- **娴嬭瘯**:`forkguard_skills_dir_unions_*`;project_context_cache / skills 澶氳矾寰勪笂娓告祴璇?`#[ignore]`
- 涓婃父 PR:skills union 鈫?[#2737](https://github.com/Hmbown/CodeWhale/pull/2737) CLOSED(涓婃父宸?harvest);constitution 鐭矾 鉂?涓撶敤

### C6 `chore` 闆剁閫傞厤
- **鏂囦欢**:`llm_client/mod.rs`銆乣core/engine/lsp_hooks.rs`銆乣lsp/mod.rs`銆乣hooks.rs`(append_file 鍏?file_write 绫?銆乣core/turn.rs`銆乣tui/app.rs`銆乣.gitignore`
- **鏀瑰姩**:缂栬瘧 / 鎺ョ嚎灞傞浂纰庨€傞厤(鍚?1-5 琛?

### C7 `prompt` static composer hook(瀵嗗皝闈欐€佸眰)
- **鏂囦欢**:`prompts.rs`銆乣core/engine/turn_loop.rs`
- **鏈哄埗**:`set_static_prompt_composer_override(Box<dyn Fn(&StaticPromptCtx)->String>)`鈥斺€攅mbedder 涓€涓?hook **鍏ㄩ噺鎺ョ缂栬瘧鏈熼潤鎬佹枃妗?*銆俙StaticPromptCtx` 鏄?pinvou3 **瀹界増**(mode/approval_mode/model_id/allow_shell/default_layers)
- **瀵嗗皝鑼冨洿**:瑁呬簡 composer 鍒欏悗缃?append 鍏?gate 鎺夆€斺€?*ContextMgmt + COMPACT_TEMPLATE + Runtime Policy Reference**(`static_prompt_composer().is_none()`)+ **per-turn `<runtime_prompt>` tag**(`static_prompt_composer_installed()`)
- **鐞嗙敱**:閫愬潡 `set_*_override` 闃蹭笉浣?涓婃父鏂板鍧楁紡杩?prompt";composer 鎶婇潤鎬佸眰瀵嗗皝,涓婃父鍗囩骇鏂?doctrine 杩涗笉浜?pinvou3 prompt
- **鈿狅笍 鍚屽悕 API 璇箟鍒嗗弶**:涓婃父鐙珛瀹炵幇**绐勭増** composer(`StaticPromptCtx{model_id, personality, default_layers}`)銆傚喅璁?鍒犱笂娓哥獎鐗堛€佷繚 pinvou3 瀹?ctx;閲囦笂娓?mode-independent 绠＄嚎浣嗗湪 `apply_static_prompt_composer` 鍐呬互甯搁噺 Yolo/Auto 鏋勯€犲 ctx銆?v0.8.60 merge:璋冪敤鐐?`effective_static_prompt_composer()` 瀵归綈 pinvou3 璁块棶鍣?`static_prompt_composer()`)
- **娴嬭瘯**:submodule `forkguard_static_prompt_composer_*`;app `forkguard_static_composer_*`
- 涓婃父 PR:[#2786](https://github.com/Hmbown/CodeWhale/pull/2786) CLOSED(涓婃父绐勭増,璇箟涓嶅悓);pinvou3 瀹界増淇?fork

### P `prefix-cache` pwd/workspace 绉诲嚭闈欐€?system(2026-06-17)
- **鏂囦欢**:`prompts.rs`(render_environment_block 鍒?`- pwd` 琛?涓?C5/C7 鍏辨鏂囦欢)銆乣core/engine.rs`(turn_metadata_block 鍔?`Current workspace` 琛?銆俛pp 灞傚悓姝?`instructions.md` 鍒?`{{PINVOU3_WORKSPACE}}`/`{{PINVOU3_DATE}}`(鏀归潤鎬佹枃妗?+ 鐩稿璺緞寮曞)銆乣bridge/mod.rs` 鍒犲搴?replace
- **鏀瑰姩**:姣?session 鍙樼殑 workspace 璺緞(pwd)浠庨潤鎬?`## Environment` **绉诲嚭** 鈫?per-turn `<turn_meta>` 鐨?`Current workspace`;date 鍚岀悊(turn_meta 鏈氨鏈?;浜у嚭寮曞鏀?鐢ㄧ浉瀵硅矾寰勫啓,宸ュ叿鑷姩钀?workspace"(瀹炴祴 4/4,PathEscape 鍏滃簳)
- **鐞嗙敱**:vLLM(寮€ `--enable-prefix-caching` + 鎶曟満瑙ｇ爜 mtp)鍦?prefix-cache **閮ㄥ垎鍛戒腑**(system 鍓嶅崐鍛戒腑涓婁釜 session銆佸埌 workspace 澶勫垎鍙夋帴缁?prefill)鏃?鍒嗗弶鐐?KV 涓嶈嚜娲?+ mtp 瀵?KV 鏁忔劅 鈫?宸ュ叿璋冪敤閫€鍖栨垚瑁?XML(瀹炴祴 L1 single subagent 25%;**鎵€鏈夊伐鍏峰彈褰卞搷**,read_file/exec_shell 閮ㄥ垎鍛戒腑涓嬪叏 0~1/4)銆傜Щ pwd/workspace 璁?static system 璺?session 瀛楄妭闈欐€?鈫?瀹屾暣鍛戒腑,25%鈫拁100%銆傗殸锔?**鐢熶骇 GUI 鏈?cache warmup 鑷垜棰勭儹瀹屾暣 prefix銆佸熀鏈笉鐘?*(B 5/6);姝?fork 涓昏淇?**L1 headless(鏃?warmup)娴嬭瘯鍑嗙‘鎬?* + 闃插尽(warmup 澶辨晥鍏滃簳)
- **娴嬭瘯**:`forkguard_environment_block_omits_volatile_pwd`(鎸囩汗鍚屽悕);L1 `subagent_single_simple`(25%鈫掔ǔ鎬?00%,13/13)+ `relpath_write_file`(鐩稿璺緞钀?workspace,4/4)
- 涓婃父 PR:鉁?**鎷熸彁**鈥斺€攑refix-cache 浼樺寲閫氱敤,涓旂鍚堜笂娓?environment-volatile 鏂瑰悜(搂8 #2314 宸?merged);PR 鎷熶负 "move volatile pwd from static system prefix to per-turn turn_meta"
> 鈿狅笍 **2026-06-18 璁㈡**:鏈妭"鐢熶骇 GUI 鏈?cache warmup 鑷垜棰勭儹"鏄?*閿欑殑**鈥斺€擿build_cache_warmup_request` 搴曞骇鍙湁鎵嬪姩 `/cache warmup` TUI 鍛戒护瑙﹀彂,pinvou3 Tauri GUI **浠庝笉鑷姩璋?*銆傜湡鐩告槸鏂?session **棣栬姹?*浠嶅喎鍚姩 脳 mtp 鈫?棣栬疆閲囨(瑙?Q 鑺?銆俻wd-move 鏈韩**涓庢紓绉绘棤鍏?*(瀹炴祴鏀?涓嶆斁閮戒笉鏄洜),淇濈暀鍗冲彲銆?

### Q `prefix-cache` session 鍚姩鑷姩 cache warmup(2026-06-18)
- **鏂囦欢**:`core/session.rs`(`Session.cache_warmup_done` 杩愯鏃舵爣蹇?銆乣core/engine/turn_loop.rs`(棣栬姹傚墠缃?warmup)
- **鏀瑰姩**:鏈?session **绗竴娆″彂璇锋眰鍓?*,鐢?*瀹屾暣鏈姹傚墠缂€(system+tools+褰撳墠杞?user 娑堟伅鍙婂叾 `<turn_meta>`)** clone 涓€涓?`max_tokens=1`/`tool_choice=none`/`stream=none`/鍝嶅簲涓㈠純鐨勯鐑姹?`await` 鍙戝嚭,鎶婃暣娈靛喎鍓嶇紑鍠傝繘 vLLM prefix-cache;涓€娆℃€?flag)銆佷笉杩?context銆?0s 瓒呮椂鍏滃簳
- **鏍瑰洜/鐞嗙敱**:vLLM(NVFP4)+ mtp 鎶曟満瑙ｇ爜鍦ㄦ柊 session **棣栬姹傚喎 prefill** 涓婃妸鐢熸垚閲囨鈥斺€旈涓?`tool_call`/`<turn_meta>` 鏍囩/绯荤粺鎸囦护琚悙鎴愯８鏂囨湰(瀹炴祴涓?session:棣栬疆婕傘€佺敤鎴?*闂竴鍙ュ嵆鑷剤**鈥斺€旀湰璐ㄥ氨鏄墜鍔?warmup)銆傗殸锔?**蹇呴』棰勭儹鍒?turn_meta**:妯″瀷鎭板湪 `<turn_meta>` 澶勫璇婚噰姝?msg1 瀹為敜 `...qwen36_35b_35b_256k...` 閲嶅),v1 鐢?`build_cache_warmup_request`(鍓ユ帀褰撳墠杞?user 娑堟伅)婕忕儹 turn_meta 鈫?浠嶆紓;v2 鐑畬鏁撮璇锋眰鎵嶆牴娌汇€?*婕傜Щ涓庡伐鍏疯〃/subagent 鏀鹃€氭棤鍏?*(鍏滃ぇ鍦堥獙璇佸悗瀹氳:鏄杞喎鍚姩,闈?schema)
- **娴嬭瘯**:`forkguard` `session warmup flag` + `棣栬姹?warmup 娉ㄥ叆` 鎸囩汗;琛屼负寰呰ˉ L1(鏂?session 棣栬疆 tool_call 涓嶆紓)
- 涓婃父 PR:鉁?**鎷熸彁**鈥斺€旀湰鍦?vLLM+mtp 鐨勯€氱敤 first-turn 闃叉紓,鑷姩 warmup 姣旀墜鍔?`/cache warmup` 鏇寸ǔ

### C8 `ops` 浼氳瘽宸ュ叿寮€鍏?SetDisallowedTools)
- **鏂囦欢**:`core/ops.rs`(鏂板 `Op::SetDisallowedTools { tools: Vec<String> }`)銆乣core/engine.rs`(handler 鍐欏叆 `config.disallowed_tools`)
- **鏀瑰姩**:杩愯鏃舵妸"琚鐢ㄥ伐鍏峰叏鍚?妯″瀷鍙,灏忓啓)"骞挎挱缁欏湪璺戝紩鎿?鈫?鍐?`config.disallowed_tools`,涓嬩竴杞?`filter_tool_catalog_for_gates` 鍗冲妯″瀷闅愯棌銆傜┖ = 涓嶇鐢?
- **鐞嗙敱**:pinvou3銆屼細璇濆伐鍏峰紑鍏炽€嶉渶瑕佹妸鐢ㄦ埛鍦?GUI 鍏虫帀鐨?connector 鍗虫椂鍚屾缁欏紩鎿?涓€旂敓鏁?;娑堣垂鏂瑰湪 pinvou3-app `engine_pool::set_disallowed_all` + `commands::set_disabled_connectors`
- **鏉ユ簮**:fork PR h3c-hexin/DeepSeek-TUI#4(宸?ff 杩?`pinvou3-clean`,commit `a0efea0b`)
- **娴嬭瘯**:`forkguard` `SetDisallowedTools op 瀹氫箟` + `SetDisallowedTools 鍐?disallowed` 鎸囩汗(L1);琛屼负 L2 寰呰ˉ
- 涓婃父 PR:鉂?pinvou3 涓撶敤(鐣?fork)

### C9 `mcp` 閰嶇疆鍊?`${ENV}` 灞曞紑(2026-06-26)
- **鏂囦欢**:`crates/tui/src/mcp.rs`
- **鏀瑰姩**:MCP `headers` 涓?stdio `env` 閰嶇疆鍊兼敮鎸?`${ENV_NAME}` 鍗犱綅绗?杩炴帴/鍚姩鍓嶄粠褰撳墠杩涚▼鐜灞曞紑;缂哄け鎴栨牸寮忛潪娉曟椂杩斿洖鍙瘖鏂敊璇€傞厤缃枃浠舵湰韬粛鍙繚瀛樺崰浣嶇銆?
- **鐞嗙敱**:pinvou3 鍐呯疆鍚岃姳椤?浼佹煡鏌?楂樺痉澶╂皵 MCP 涓嶈兘鍐嶆妸渚涘簲鍟?API Key 鏄庢枃鍐欒繘 `~/.pinvou3/bundle/mcp.json`銆侾invou app 灞傛妸鐪熷疄瀵嗛挜鏀捐繘绯荤粺鍑嵁瀛樺偍,杩愯鏃跺悓姝ュ埌杩涚▼鐜;搴曞骇鍙礋璐ｆ妸 mcp.json 涓殑 `${PINVOU3_MCP_SECRET_*}` 灞曞紑鍚庝紶缁?MCP HTTP headers 鎴?stdio 瀛愯繘绋嬬幆澧冦€?
- **瀹夊叏杈圭晫**:涓嶄粠鐖惰繘绋嬮€忎紶浠绘剰 `*_API_KEY`;鍙睍寮€閰嶇疆鏂囦欢鏄惧紡寮曠敤鐨勫彉閲忋€傛棩蹇楀拰 spawn 閿欒浠嶅彧鎵撳嵃 env key 鍒楄〃,涓嶆墦鍗板€笺€?
- **娴嬭瘯**:鏂板 `mcp.rs` 鍗曟祴瑕嗙洊 header/env 灞曞紑涓庣己澶卞彉閲忔姤閿?pinvou3-app marketplace 娴嬭瘯瑕嗙洊 mcp.json 涓嶈惤鏄庢枃銆?
- 涓婃父 PR:鉁?鍊欓€夈€侰laude/Codex/OpenCode 椋庢牸 mcp config 涓父瑙?`${TOKEN}` 鍗犱綅,涓婃父娉ㄩ噴宸叉彁绀?v0.8.31 涓嶆浛鎹?瀹炵幇閫氱敤涓旇兘鍑忓皯 mcp.json 鏄庢枃瀵嗛挜銆?

- **Verification**: `cargo test --manifest-path DeepSeek-TUI/crates/tui/Cargo.toml expand_env_placeholders --lib` PASS (3 passed, 2026-06-26).
### R `agentic-rag` EngineConfig.extra_tools 搴旂敤灞傚伐鍏锋敞鍏ュ彛(2026-06-24)
- **鏂囦欢**:`core/engine.rs`(`ExtraTools` newtype + `EngineConfig.extra_tools` 瀛楁 + Default)銆乣core/engine/tool_setup.rs`(`build_turn_tool_registry_builder` 鏈熬 `with_tool` 寰幆娉ㄥ唽);杩炲甫琛?3 澶?TUI 璺緞 EngineConfig literal(`runtime_threads.rs`/`tui/ui.rs`/`main.rs`,`extra_tools: Default::default()`)
- **鏀瑰姩**:缁?`EngineConfig` 鍔?`pub extra_tools: ExtraTools`(newtype 鍖?`Vec<Arc<dyn ToolSpec>>`,鎵嬪啓 Debug 杈撳嚭宸ュ叿鍚嶁€斺€擿dyn ToolSpec` 闈?Debug,鍚﹀垯鐮?`#[derive(Debug)]`),姣?turn build registry 鏃?append 鍒?builder銆傝**宓屽叆搴旂敤**(pinvou3-app)鏃犻渶 fork 宸ュ叿琛ㄥ嵆鍙敞鍐岃嚜瀹氫箟 `ToolSpec`
- **鐞嗙敱/鐢ㄩ€?*:Agentic RAG鈥斺€攁pp 灞?`KbSearchTool`(`knowledge/kb_tool.rs`,鎸?`session_id`,execute 鏌ヨ浼氳瘽鎸傝浇鐭ヨ瘑闆?鈫?`L1Store::retrieve_for_chat`)缁忔娉ㄥ叆,璁╂湰鍦?LLM 鑷富璋?`kb_search` 妫€绱㈡湰鍦扮煡璇?鏇夸唬鏃ф敞鍏ュ紡)銆俙spawn_for_session` 鎸?session push,宸ュ叿鎸?session_id 瑙ｅ喅 `ToolContext` 鏃?session_id 鐨勯棶棰?
- **娴嬭瘯**:`forkguard` `RAG1 extra_tools 瀛楁` + `RAG2 tool_setup 娉ㄥ唽` 鎸囩汗;app lib `blocklist_contract`(kb_search 鍙)+ `kb_tool::tests`;鐪熸満娴嬭嚜鍙戣皟鐢ㄧ巼/骞昏鐜?
- 涓婃父 PR:鉁?**鎷熸彁**鈥斺€擿extra_tools` 鏄€氱敤鎵╁睍鐐?浠讳綍宓屽叆鏂瑰彲娉ㄥ唽宸ュ叿),涓庡叿浣?kb_search 瑙ｈ€?

### W `workflow` 涓夌渷鍏儴宸ヤ綔娴佸簳搴у眰
- **鏂囦欢**:`tools/subagent/{mod,tests}.rs`銆乣core/ops.rs`銆乣core/events.rs`銆乣core/engine.rs`銆乣core/engine/{tests,approval,handle}.rs`銆乣tools/user_input.rs`銆乣runtime_threads.rs`銆乣tui/{sidebar,command_palette,ui,views/mod}.rs`銆乣main.rs`(EngineConfig 瀛楁)
- **瀛?patch**:

  | | 鍐呭 |
  |---|---|
  | W1 | `Op::SpawnSubAgent` +role_id/allowed_tools/max_steps/output_schema/expects_file_output;engine 鎸夎鑹茬櫧鍚嶅崟+姝ユ暟娲?Custom SubAgent;绌虹櫧鍚嶅崟 fail-fast |
  | W2/W3/W11 | StructuredOutput:`submit_output` 宸ュ叿 + schema 鏍￠獙 + x-output-file 钀界洏;鍌氦閲嶈瘯涓婇檺(`MAX_STRUCTURED_OUTPUT_RETRIES`),鑰楀敖缃?failed;**缁撴瀯鍖栦骇鍑鸿惤鐩樻垚鍔熷嵆 break**(鍚﹀垯 temp=0 姘稿姩) |
  | W4 | `request_user_input` 绛旀鎬荤嚎璺敱缁?SubAgent(`user_input_tx`,涓嶅悆 TOOL_TIMEOUT) |
  | W5 | `AgentComplete` +role(SDAN)+failed(瀹夸富璧板け璐ヨ矾寰?涓嶈闄堟棫浜х墿娲楁垚 PASS) |
  | W6 | SubAgent Mailbox(TokenUsage 绛変俊灏佺洿杈惧涓?+ AgentSpawned 鍏宠仈 agent_id鈫抮ole_id |
  | W7 | 璐績瑙ｇ爜:SubAgent 姣忔 `temperature=0`(鏍规不 NVFP4 涓嬪伐鍏疯皟鐢?XML 琚噰姝啋绌鸿浆) |
  | W8 | SubAgent surface 娉ㄥ唽 web/custom 宸ュ叿 |
  | W9 | ~~read_pdf catch_unwind 闃?panic~~ **v0.8.60 琚笂娓?`guard_pdf_extract` harvest**(瑙?搂2.2) |
  | W10 | `EngineConfig.reasoning_effort` 浼氳瘽寤烘椂鍒濆鍖?涓嶄緷璧栭鏉?SendMessage);`"off"` 鐢?app bridge 鎸?`provider==vllm` 娉ㄥ叆 |
  | W12 | `SubAgentSpawnOptions.max_steps` per-spawn 瑕嗙洊(`options.max_steps.unwrap_or(self.max_steps)`),registry 鐨?15/20/30 鐪熺敓鏁?|
- **tool_whitelist**(涓?C2 blocklist **浜掕ˉ涓ゅ眰,涓嶅啿绐?*):`EngineConfig.tool_whitelist` 閫氱敤鐧藉悕鍗曟満鍒?submodule 瀛楁 + turn_loop `retain`)銆俠locklist 鍏ㄥ眬鍑忔硶(寤?catalog 鏃?;tool_whitelist per-session `retain`(turn_loop 鏈€鍚?銆倃hitelist 鍦?blocklist 杩囨护鍚庣殑闆嗕笂 retain 鈫?**鏃犳硶閲嶆柊鏆撮湶榛戝悕鍗曞伐鍏?*銆傗殸锔?**app 灞傜洃宸ョ敤娉曞凡鍒?2026-06-15,瀵硅瘽鍨嬬洃宸ュ簾寮?**:`supervisor_tool_whitelist()` + `spawn_for_session` 鏂藉姞 + 姝讳唬鐮?`build_engine_config_for_workflow` 鍧囩Щ闄?**鏈哄埗鏈韩(submodule)淇濈暀寰呯敤,瀛楁鎭?None**;submodule `engine.rs:263` doc 浠嶆湁涓€澶勬寚鍚戝凡鍒犲嚱鏁扮殑鎮┖寮曠敤,寰呬笅娆?sync 椤哄甫娓呫€?
- **楠岃瘉**:L1 subagent scenarios 鐪?vLLM 璺戦€?`subagent_compare_3_libs` 骞惰 3 agent / 487s);W1鈥揥12 forkguard 鎸囩汗;琛屼负灞備粎 W10 `engine_config_locks_critical_fields`
- 涓婃父 PR:鉂?pinvou3 涓撶敤(鍙鐢ㄤ笂娓?WhaleFlow 鍩虹 crate,鏆傛湭杩?

### app 灞?fork(涓嶅湪 submodule 鈥斺€?override hook / bridge 娉ㄥ叆,fork-guard 涔熷畧)
- **prompt 鍐呭(鍗曚竴鏉ユ簮,main #14 閲嶆瀯 2026-06-15)**:`resources/bundle/instructions.md` 鏄?*鍞竴 pinvou3 prompt 鏉ユ簮**鈥斺€斿娉?瑁佸喅/`AUTHORITY_RECAP` 鍏ㄦ姌鍙犺繘 搂搴曠嚎 + 鍔ㄦ€佹敞鍏?`{{PINVOU3_MODEL}}`/`{{PINVOU3_DATE}}`(娌?缂栨椂闂?);`bridge/bundle.rs` 鍙墿 Mode 鍧?+ `LOCALE_PREAMBLE/CLOSER` zh+ja 鐭増(`AUTHORITY_RECAP=""`銆乥ase.md 鐣欑┖ stub銆乣compose_static_layers` 涓?base 鍙墿 Mode,**Plan 妯″紡浠嶆寜 mode 鍒?*)銆傜粡 `set_*_override` + `set_static_prompt_composer_override` 娉ㄥ叆銆?*submodule 鍐?prompt 鏂囨 drift=0**銆備緷鎹?ablation 瀹炴祴(user memory `prompt-ablation-methodology`):base.md 瀵?Qwen3.6 鍙祴浠峰€间粎 Voice;鏁?prompt 22590鈫?6612B,鍓╀綑澶уご=Skills~52%(`~/.agents/skills` 鍏ㄥ眬 lark 鎶€鑳?寰呴噸璁捐)
- **bridge config**(`bridge/mod.rs`):`subagent_api_timeout=300`銆乣max_subagents`(prefs 榛樿)銆乣network_policy` fake-ip CIDR(`198.18.0.0/15`)銆乣compaction.token_threshold=190_000`(256K脳74%)銆乣InstructionSource::Inline`銆倂0.8.58-60 鏂板瓧娈?verbosity/interactive_launch_limit/goal_*/disallowed_tools)鍏ㄩ€忎紶 default
- **鏁忔劅鐩綍 deny hook**:`resources/bundle/deny_sensitive_paths.sh`鈥斺€擳oolCallBefore 鎷︽晱鎰熻矾寰?+ 鍏抽棴鎬?sudo銆?*hard-deny 蹇呴』 `exit 2`**(v0.8.60 Hooks v2 `fold_tool_call_before_results` 鍙 exit_code==2,鏃?exit 1 琚綋 passthrough)
- **dump 宸ュ叿**:`bin/dump_system_prompt.rs`(闅?`PromptSessionContext` 瀛楁 / prompt 鍑芥暟绛惧悕缁存姢)

---

## 2. 绉婚櫎 / harvest 娓呭崟

### 2.1 clean re-fork 姘镐箙涓㈠純(2026-06-04,涓嶅啀甯﹀叆)
- subagent 鏈湴绾︽潫鍏ㄥ(MAX_STEPS/ELAPSED/resolve_agent_ref/tool_agent_route)鈥斺€擿agent_*`/`delegate` 鍏ㄥ湪 blocklist,鐢熶骇涓嶅彲杈?
- phase/demo workflow(璺ㄤ粨鍏ㄥ垹)鈥斺€斿凡鐢?W 涓夌渷鍏儴閲嶅仛
- qwen-128K 姝荤爜(models.rs)鈥斺€旂湡瀹炴ā鍨嬭蛋涓婃父 `_Nk` hint

### 2.2 宸茶涓婃父 harvest(鎸囩汗鎾ら櫎,闈?fork-distinct)
- **v0.8.53 鍙婁互鍓?*:bing decode銆乶etwork_policy fake-ip API銆両nstructionSource enum銆乥ase override hook銆丒ngineConfig.instructions銆?56K auto-compact 鍩虹璁炬柦銆丮AX_OUTPUT env銆乫ile_search/grep_files timeout
- **v0.8.57**:skills union 鎺ョ嚎銆丆4-a 澶氳閫愯(`split_command_segments`)銆佹湰鍦?Bocha(#2946)
- **v0.8.60**:**W9 read_pdf catch_unwind** 鈫?涓婃父 `guard_pdf_extract`(`file.rs`,鍚岃涔?catch_unwind+閿欒鏄犲皠,甯﹁嚜娴?char-boundary 閮ㄥ垎涔熷凡鏄笂娓歌嚜甯?銆備唬浠蜂粎缃曡 font/CMap panic 鐨勪腑鏂囨彁绀?

---

## 3. fork-guard 瀹堟姢 + sync 鍚庨獙璇?

```bash
./scripts/fork-guard.sh          # 鍏ㄩ噺:鎸囩汗灞?+ 缂栬瘧璺戝洖褰掓祴璇?
./scripts/fork-guard.sh --fast   # 浠呮寚绾瑰眰,绉掔骇(merge 鍚庣涓€閬撳揩绛?
```

涓ゅ眰:**鎸囩汗灞?* grep 姣忎釜 fork 鏍囪鏄惁杩樺湪(鎶撱€宮erge 闈欓粯涓㈡暣娈?patch銆?;**琛屼负灞?* `cargo test` 璺戝洖褰掓祴璇?鎶撱€屽€?閫昏緫琚敼鍥炰笂娓搞€?銆?*43 鎸囩汗**(submodule C1-C7+W+P / app),瀹屾暣娓呭崟瑙?`fork-guard.sh` `fingerprints=` 鏁扮粍鈥斺€旀柊澧?fork patch 蹇呭悓姝ュ姞鎸囩汗(瑙?fork-policy 搂3)銆?

### 鈿狅笍 sync 鍚庡繀鍋氶獙璇?checklist(fork-guard **涓嶅**,姣忔潯閮借俯杩囧潙)
1. **鍏ㄩ噺 lib 娴嬭瘯** `cargo test -p codewhale-tui --lib`鈥斺€旀姄闈?`forkguard_` 鍓嶇紑鐨勪笂娓告祴璇曞洜 fork fail(v0.8.51 append_file 闈欓粯涓㈠け闈犳鎶?
2. **dump_system_prompt 鍓嶅悗 diff**(涓嶅湪 fork-guard 鏋勫缓閲?鈥斺€旈潪 0 灏遍€愬潡鏌ヨ皝婕忚繘闈欐€?prompt(v0.8.57 Runtime Policy 141 琛屾硠婕忛潬姝ゆ姄)
3. **鎵?per-turn message 鏋勯€犺矾寰?* `grep -rn "runtime_prompt\|messages.push" turn_loop.rs engine.rs`鈥斺€斾笂娓稿彲鑳芥柊澧炴瘡璇锋眰娉ㄥ叆鐨?transient 娑堟伅,dump 鎶撲笉鍒?
4. **宸ュ叿闆嗗悎 + 婵€娲绘満鍒剁洏鐐?*:鈶?瀵规瘮涓ょ増 `ToolSpec::name()` 闆嗗悎,鏂板伐鍏锋紡鍏ヨ琛ラ粦鍚嶅崟;鈶?**鏇磋鏌ヤ笂娓告湁娌℃湁鏂板鑳芥縺娲?deferred 宸ュ叿鐨勬満鍒?*(`tool_search`/`ensure_advanced_tooling` 绫?鈥斺€攂locklist 鏄?defer 闈炲垹闄?浠讳綍婵€娲?deferred 鐨勬柊璺緞閮藉嚮绌块棬鎺?
5. **hook 鍐崇瓥鍗忚**:涓婃父鍙兘鏀?hook 閫€鍑虹爜/JSON 濂戠害(v0.8.60 Hooks v2 鎶?hard-deny 浠庛€岄潪闆躲€嶆敼鎴愩€宔xit 2銆?鈥斺€攄ump/缂栬瘧閮芥姄涓嶅埌,蹇呴』璇?`fold_tool_call_before_results` 纭 deny 鑴氭湰閫€鍑虹爜濂戠害
6. **app 绔崟绾跨▼娴嬭瘯** `cargo test --manifest-path pinvou3-app/.../Cargo.toml --lib -- --test-threads=1`鈥斺€攂ridge env 娴嬭瘯骞惰浼?flake(闈炲洖褰?

---

## 4. Sync 鍘嗗彶

### Clean re-fork(2026-06-15,HEAD `1161bc78`)
绗?2 娆?clean re-fork(棣栨 2026-06-04 鈫?v0.8.53)銆傚姩鏈?v0.8.60 merge 鍚庡巻鍙蹭贡(26 commit / ~10 merge / fork 鏁ｈ惤涓夋 sync)銆?
- **鍋氭硶**:`git reset --soft v0.8.60` 淇濈暀鍏ㄩ儴 fork 鏍?鈫?鎸?file鈫抰heme 閲嶇粍鎴?8 涓嚎鎬т富棰?commit,**鏈€缁堟爲涓?merge `fa412ca1` 瀛楄妭绛変环**(fork-guard 41 鎸囩汗鍏ㄨ繃)銆傚浠?`backup/pre-reclean-v0.8.60`
- **閫?patch 璇勪及**:鍏ㄩ儴 C1-C7+W 閮藉湪鐢?L1 21/21 + L2 166 + forkguard 楠岃瘉),鏃犳椿浠ｇ爜鍙垹;C4 璇勪及涓虹暀(YOLO 闃茬伨闅剧綉);tool_whitelist鈫攂locklist 涓嶅啿绐?浜掕ˉ涓ゅ眰);Plan 妯″紡灞?app 灞傜嫭绔嬫竻鐞?鏈涓嶅姩
- **娓呯悊**:shell.rs 鍒犲凡鎺ㄧ炕鐨?`鍢存浛璁捐.md` 寮曠敤;engine.rs 鍘昏繃鏃跺搧鎮熷紩鐢?+ 鍔?tool_whitelist 涓ゅ眰妯″瀷 doc

### v0.8.60(2026-06-15,merge v0.8.57鈫抳0.8.60,279 commit / 248 鏂囦欢)
**澶х増鏈?sync**銆備笂娓镐富绾?Native Anthropic provider銆?*Hooks v2(JSON allow/deny/ask 鍐崇瓥濂戠害)**銆丄gent Fleet 鐪熻窇銆?goal 鐩爣绠＄悊銆乧oncise verbosity銆乮nteractive fanout 闂搞€佸 provider/model銆佸懡浠ら噸鏋勬垚 `commands/groups/`銆乧onstitution prompt 鏀?YAML+renderer銆?
- **鍐茬獊闈㈠皬**:248 鏂囦欢浠?**7 鏂囦欢 / 14 鍐茬獊鍧?*(鍏朵綑鑷姩鍚堝苟)銆傝:prompts.rs(C7 娴嬭瘯 + 璁块棶鍣ㄥ悕瀵归綈)/ turn_loop.rs(C7 gate)/ project_context.rs(C5 鐮嶇┖淇?None)/ subagent/mod.rs(W union)/ subagent/tests.rs(W union)/ file.rs(W9 harvest)/ main.rs(EngineConfig 瀛楁)
- **馃敶 鎶撳埌鐪熷畨鍏ㄥ洖褰?*:`deny_sensitive_paths.sh` 闈?**exit 1** 鎷掔粷,浣?Hooks v2 鏀规垚鍙 `exit_code==2`(exit 1 褰?ALLOW)鈫?纭闈欓粯澶辨晥銆備慨:鍏ㄦ敼 exit 2 + fork-guard 鍔犳寚绾?
- **app 閫傞厤**:EngineConfig/Op/dump 琛?5+3+2 涓柊瀛楁(GUI 鍏ㄩ€忎紶 default);lib.rs 鍔?`pub mod fleet/context_report/model_inventory`銆佸垹瀛ゅ効 `prompt_persist`
- **楠岃瘉**:dump 瀛楄妭绋冲畾銆乥locklist 鏃犻渶鏀?鏃犳柊 model 宸ュ叿)銆乫ork-guard 鍏ㄨ繃銆乴ib 4539/app 166 pass銆丩1 21/21
- **鏁欒**:鈶?涓婃父鍚岃涔?API 鍚嶅樊寮?`effective_static_prompt_composer`)merge 鍙栦笂娓稿悕,缂栬瘧鑳芥姄;鈶?**hook 鍐崇瓥鍗忚鍙樻洿鏄?dump/缂栬瘧閮芥姄涓嶅埌鐨勯殣褰㈠畨鍏ㄥ洖褰掆€斺€斿繀椤昏 fold 閫昏緫**;鈶?澶х増鏈彿宸墵灏?diff,commit/鏂囦欢鏁版墠鏄湡瑙勬ā

### v0.8.57(2026-06-11,merge v0.8.53鈫抳0.8.57,342 commit)
DeepSeek鈫扖odeWhale rebrand + **system prompt 鏀?mode-independent**(mode/approval 绉诲嚭闈欐€佸墠缂€璧?per-turn `<runtime_prompt>` tag)銆傚叧閿垽鏂?C7 composer 鍚屽悕 API 璇箟鍒嗗弶(淇濆 ctx)銆丷untime Policy + runtime_prompt tag 涓ら亾鏂?gate(#42)銆?*tool_search 鍑荤┛ blocklist**(涓婃父鏂版敞鍏ヨ矾寰勬縺娲?deferred agent 宸ュ叿 鈫?鍓嶇瑁?JSON;闈?`spawn_headless` probe 鐪熷疄閾捐矾瀹氫綅,淇硶瑙?C2)銆?

### 鏃х増鏁欒閫熸煡(v0.8.47鈥?3,per-conflict 缁嗚妭宸插簾寮?
| 鐗堟湰 | 鍙鐢ㄦ暀璁?|
|---|---|
| v0.8.53 | dump bin 涓嶅湪 fork-guard 鏋勫缓閲?**sync 鍚庡崟璺?*(`PromptSessionContext` 婕忓瓧娈甸潬瀹冩姄) |
| v0.8.51 | **sync 鍚庡繀璺戝叏閲?lib 娴嬭瘯**(merge 鍙栦笂娓?`Implementer.allowed_tools` 闈欓粯涓?append_file) |
| v0.8.49 | **鏁存枃浠?`--theirs` 鍗遍櫓**(鍐叉帀涓嶅湪鍐茬獊鍖虹殑 fork patch)鈫?fork-distinct 鏂囦欢閫?hunk 瑙?|
| v0.8.47 | 涓婃父鎶婂伐鍏?deferral 缈绘垚 allowlist(`request_user_input` 琚?defer 姘旀场娑堝け)鈫?C2 鐨勭敱鏉?|

### app 灞?prompt 鐦﹁韩(2026-06-05,20.2K鈫?.9K,杩唬 prompt 鍓嶅繀璇?
鍙嶄簨瀹炲璁?銆屾病瀹冨摢鏉＄敓浜ц矾寰勪細鍙樸€?鍒?Personality(骞跺叆 base.md 搂Voice)/ Session Longevity(涓?blocklist 鐭涚浘)/ Approval Policy(鍗?Yolo-Auto)/ prompt-cache 鏁欏 / Compaction Relay 妯℃澘(鏃犵敓浜ц€呮棤娑堣垂鑰?/ Article VII 涔濆眰鈫掍笁琛岃鍐?/ Sub-agents(宸ュ叿涓嶅彲瑙?銆傛搷浣滄€у師鍒欏綊 instructions.md 鍗曚竴鏉ユ簮,base.md 鍙暀绾㈢嚎+瑁佸喅+璇皵銆?
