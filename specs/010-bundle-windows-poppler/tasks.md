# 浠诲姟锛歐indows 鍐呯疆 Poppler 瀹夎

**杈撳叆**锛歚specs/010-bundle-windows-poppler/` 涓嬬殑璁捐鏂囨。銆?
**鍓嶇疆鏉′欢**锛歚plan.md`銆乣spec.md`銆乣research.md`銆乣data-model.md`銆乣contracts/`銆乣quickstart.md`

**娴嬭瘯**锛氭湰 feature 娑夊強 Windows 鎵撳寘銆侀檮浠惰В鏋愬拰渚濊禆浣撴锛屼换鍔″寘鍚笌椋庨櫓鍖归厤鐨?Rust 娴嬭瘯銆侀厤缃鏌ュ拰 MSI 鎵嬪姩楠屾敹銆?
**缁勭粐鏂瑰紡**锛氫换鍔℃寜鐢ㄦ埛鏁呬簨鍒嗙粍锛岀‘淇濇瘡涓晠浜嬪彲鐙珛瀹炵幇鍜岄獙璇併€?
## 鏍煎紡锛歚[ID] [P?] [Story] 鎻忚堪`

- **[P]**锛氬彲骞惰鎵ц锛屽墠鎻愭槸淇敼涓嶅悓鏂囦欢涓旀病鏈変緷璧栧叧绯汇€?- **[Story]**锛氱敤鎴锋晠浜嬩换鍔′娇鐢?`[US1]`銆乣[US2]`銆乣[US3]` 鏍囪銆?- 姣忎釜浠诲姟鎻忚堪閮藉寘鍚槑纭枃浠惰矾寰勬垨楠岃瘉鍛戒护銆?
## Phase 1: 鍑嗗锛堝叡浜熀纭€锛?
**鐩殑**锛氱‘璁ゅ綋鍓?feature 涓婁笅鏂囥€佽祫婧愭潵婧愬拰宸ヤ綔鍖鸿竟鐣屻€?
- [X] T001 闃呰 `specs/010-bundle-windows-poppler/plan.md` 骞剁‘璁ゅ绔犳鏌ョ粨鏋滀负 PASS
- [X] T002 妫€鏌?`git status --short` 骞惰褰曞綋鍓嶄粎鏈?Spec Kit artifacts/AGENTS.md 鐩稿叧鏀瑰姩锛岄伩鍏嶈鐩栫敤鎴锋湭鎻愪氦淇敼
- [X] T003 [P] 纭 Poppler 婧愮洰褰?`C:\Users\z27014\Downloads\poppler-26.02.0\` 瀛樺湪涓斿寘鍚?`pdftotext.exe`銆乣pdftoppm.exe` 鍜屼緷璧?DLL
- [X] T004 [P] 闃呰 `pinvou3-app/src-tauri/tauri.conf.json` 涓?Tauri 2 bundle 鏂囨。锛岀‘璁?`resources` 鍒板畨瑁呯洰褰?`poppler` 鐨勯厤缃啓娉?
---

## Phase 2: 鍩虹浠诲姟锛堥樆濉炲悗缁晠浜嬶級

**鐩殑**锛氬缓绔嬫墍鏈夌敤鎴锋晠浜嬪叡鍚屼緷璧栫殑璧勬簮鐩綍銆丱S 鎶借薄鍜岄獙璇佸叆鍙ｃ€?
**鈿狅笍 CRITICAL**锛氭湰闃舵瀹屾垚鍓嶏紝涓嶅簲寮€濮嬩换浣曠敤鎴锋晠浜嬪疄鐜般€?
- [X] T005 鍒涘缓 `pinvou3-app/src-tauri/resources/windows/poppler/` 骞朵粠 `C:\Users\z27014\Downloads\poppler-26.02.0\` 澶嶅埗瀹屾暣 Poppler 杩愯鏃舵枃浠?- [X] T006 鍦?`pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 涓柊澧?Windows 瀹夎鐩綍 Poppler 璺緞瑙ｆ瀽 helper锛岃繑鍥?`{褰撳墠鍙墽琛屾枃浠剁洰褰晑/poppler`
- [X] T007 鍦?`pinvou3-app/src-tauri/src/os/windows/mod.rs` 涓?`pinvou3-app/src-tauri/src/os/interface/mod.rs` 涓鍑?PDF 宸ュ叿璺緞瑙ｆ瀽鎺ュ彛
- [X] T008 鍦?`pinvou3-app/src-tauri/src/os/linux/mod.rs` 涓?`pinvou3-app/src-tauri/src/os/unsupported.rs` 涓ˉ榻愬悓鍚?PDF 宸ュ叿璺緞瑙ｆ瀽闄嶇骇瀹炵幇锛屼繚鎸侀潪 Windows 缂栬瘧閫氳繃
- [X] T009 [P] 鍦?`pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 娣诲姞鍗曞厓娴嬭瘯瑕嗙洊瀹夎鐩綍鍖呭惈绌烘牸鍜屼腑鏂囧瓧绗︽椂鐨?Poppler 璺緞鎷兼帴
- [X] T010 [P] 鍦?`specs/010-bundle-windows-poppler/quickstart.md` 涓ˉ鍏呮渶缁堥噰鐢ㄧ殑 Tauri resource/MSI 楠岃瘉鍛戒护

**妫€鏌ョ偣**锛歄S 灞傝兘琛ㄨ揪鈥滃唴缃?Poppler 璺緞鈥濓紝璧勬簮鐩綍宸插瓨鍦紝鍚庣画鏁呬簨鍙嫭绔嬫帹杩涖€?
---

## Phase 3: 鐢ㄦ埛鏁呬簨 1 - Windows 瀹夎鍚?PDF 鑳藉姏寮€绠卞彲鐢?(Priority: P1) 馃幆 MVP

**鐩爣**锛歐indows 鐢ㄦ埛瀹夎鍚庢棤闇€鎵嬪姩瀹夎 Poppler锛屽嵆鍙笂浼犳枃瀛楀眰 PDF 骞惰В鏋愩€?
**鐙珛娴嬭瘯**锛氬湪鏈厤缃?Poppler PATH 鐨?Windows 鐜涓繍琛屽簲鐢ㄦ垨瀹夎鐗堬紝涓婁紶鏂囧瓧灞?PDF锛岀‘璁?`pdftotext` 浣跨敤鍐呯疆 Poppler 骞朵骇鐢熷彲鍙戦€佹枃鏈唴瀹广€?
### 娴嬭瘯 / 楠岃瘉

- [X] T011 [P] [US1] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 娣诲姞娴嬭瘯锛岄獙璇?PDF 宸ュ叿鍛戒护閫夋嫨鍙帴鏀?OS 灞傝繑鍥炵殑缁濆璺緞
- [X] T012 [P] [US1] 鍦?`pinvou3-app/src-tauri/src/os/windows/windows_path.rs` 娣诲姞娴嬭瘯锛岄獙璇佸唴缃?`pdftotext.exe` 浼樺厛浜庣郴缁?PATH 鍛戒护

### 瀹炵幇

- [X] T013 [US1] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 涓皢 PDF 鏂囨湰鎻愬彇鍛戒护浠庣‖缂栫爜 `pdftotext` 鏀逛负璋冪敤 OS 灞?PDF 宸ュ叿瑙ｆ瀽鎺ュ彛
- [X] T014 [US1] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 涓皢鎵弿浠?PDF 鍏滃簳浣跨敤鐨?`pdftoppm` 鏀逛负璋冪敤 OS 灞?PDF 宸ュ叿瑙ｆ瀽鎺ュ彛
- [X] T015 [US1] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 涓洿鏂?Windows 涓嬪唴缃?Poppler 缂哄け鏃剁殑 PDF 涓婁紶閿欒鏂囨锛屾寚鍚戝畨瑁呭唴瀹瑰紓甯告垨淇瀹夎
- [X] T016 [US1] 杩愯 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 骞惰褰曠粨鏋滃埌 `specs/010-bundle-windows-poppler/quickstart.md`

**妫€鏌ョ偣**锛歎S1 鍙嫭绔嬫紨绀哄拰楠岃瘉锛涗笉渚濊禆渚濊禆浣撴 UI 鏄惁宸叉洿鏂般€?
---

## Phase 4: 鐢ㄦ埛鏁呬簨 2 - 瀹夎鍖呮惡甯﹀彈鎺?Poppler 杩愯鏃?(Priority: P2)

**鐩爣**锛歐indows MSI 鎼哄甫 Poppler锛屽苟鍦ㄥ畨瑁呭悗閲婃斁鍒?`{瀹夎鐩綍}/poppler`銆?
**鐙珛娴嬭瘯**锛氭瀯寤?Windows MSI锛屽湪骞插噣鐜瀹夎鍚庢鏌ュ畨瑁呯洰褰曚腑瀛樺湪 `poppler\pdftotext.exe` 鍜?`poppler\pdftoppm.exe`銆?
### 娴嬭瘯 / 楠岃瘉

- [X] T017 [P] [US2] 鍦?`pinvou3-app/src-tauri/resources/windows/poppler/` 澧炲姞璧勬簮瀹屾暣鎬ф鏌ヨ褰曪紝鍒楀嚭鑷冲皯 `pdftotext.exe`銆乣pdftoppm.exe` 鍜屽叧閿?DLL
- [X] T018 [P] [US2] 鍦?`specs/010-bundle-windows-poppler/quickstart.md` 澧炲姞 MSI 瀹夎鍚庢鏌?`{瀹夎鐩綍}/poppler` 鐨?PowerShell 鍛戒护

### 瀹炵幇

- [X] T019 [US2] 鍦?`pinvou3-app/src-tauri/tauri.conf.json` 涓惎鐢?Windows MSI 鎵撳寘鐩爣锛屽悓鏃朵繚鐣欑幇鏈?Linux deb 鐩爣
- [X] T020 [US2] 鍦?`pinvou3-app/src-tauri/tauri.conf.json` 涓厤缃?`pinvou3-app/src-tauri/resources/windows/poppler/` 闅?Windows MSI 瀹夎鍒板簲鐢ㄥ畨瑁呯洰褰曚笅鐨?`poppler`
- [X] T021 [US2] 鍦?`pinvou3-app/src-tauri/tauri.conf.json` 鎴?Windows bundle 閰嶇疆涓姞鍏?`{瀹夎鐩綍}/poppler` 鐨?PATH/鐜鍙橀噺瀹夎绛栫暐
- [ ] T022 [US2] 鏋勫缓 Windows MSI 骞跺湪瀹夎鍚庢墽琛?`Get-ChildItem "<瀹夎鐩綍>\\poppler\\pdftotext.exe"` 涓?`Get-ChildItem "<瀹夎鐩綍>\\poppler\\pdftoppm.exe"` 楠岃瘉璧勬簮閲婃斁
- [ ] T023 [US2] 鍦ㄦ湭棰勮 Poppler 鐨?Windows 鐜瀹夎 MSI 鍚庝笂浼犳枃瀛楀眰 PDF锛屾寜 `specs/010-bundle-windows-poppler/quickstart.md` 璁板綍 smoke 缁撴灉

**妫€鏌ョ偣**锛歁SI 瀹夎浜х墿鍙嫭绔嬭瘉鏄?Poppler 宸查殢搴旂敤瀹夎銆?
---

## Phase 5: 鐢ㄦ埛鏁呬簨 3 - 渚濊禆浣撴涓嶅啀瑕佹眰鐢ㄦ埛琛?Poppler (Priority: P3)

**鐩爣**锛歐indows 渚濊禆浣撴涓嶅啀鏄剧ず Poppler/PDF 鏂囨湰鎻愬彇缂哄け椤癸紝鍏朵粬渚濊禆椤逛繚鎸佹甯搞€?
**鐙珛娴嬭瘯**锛氬湪 Windows 搴旂敤涓墦寮€渚濊禆浣撴锛岀‘璁ゆ病鏈?Poppler/PDF 鏂囨湰鎻愬彇鎵嬪姩瀹夎鎻愮ず锛涘悓鏃?Tesseract銆丳andoc銆丩ibreOffice 绛夊叾浠栫己澶遍」浠嶆寜鍘熻鍒欏睍绀恒€?
### 娴嬭瘯 / 楠岃瘉

- [X] T024 [P] [US3] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 娣诲姞渚濊禆浣撴娴嬭瘯锛岄獙璇?Windows 骞冲彴杩囨护 Poppler/PDF 鏂囨湰鎻愬彇椤逛笖淇濈暀鍏朵粬渚濊禆椤?- [X] T025 [P] [US3] 鍦?`pinvou3-app/src/index.html` 鎴栧墠绔墜鍔ㄩ獙鏀惰褰曚腑楠岃瘉渚濊禆浣撴鍒楄〃涓嶄細灞曠ず Poppler 鎵嬪姩瀹夎鎻愮ず

### 瀹炵幇

- [X] T026 [US3] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 鐨?`check_dependencies` 涓寜骞冲彴闅愯棌 Windows Poppler/PDF 鏂囨湰鎻愬彇椤?- [X] T027 [US3] 鍦?`pinvou3-app/src-tauri/src/file_ingest.rs` 涓繚鎸?Linux 鐨?`poppler-utils` 渚濊禆浣撴琛屼负涓嶅彉
- [X] T028 [US3] 鍦?`pinvou3-app/src/index.html` 鍜?`pinvou3-app/src/tauri-bridge.js` 涓‘璁や緷璧栦綋妫€ UI 涓嶅啀鍋囪 Poppler 椤瑰繀鐒跺瓨鍦?- [ ] T029 [US3] 杩愯 `cargo test --manifest-path pinvou3-app/src-tauri/Cargo.toml file_ingest --lib` 骞舵墜鍔ㄦ墦寮€ Windows 渚濊禆浣撴瀹屾垚 UI 楠屾敹

**妫€鏌ョ偣**锛歐indows 渚濊禆浣撴琛屼负鍙嫭绔嬮獙璇侊紱Linux 琛屼负鏈洖褰掋€?
---

## Phase 6: 鏀跺熬涓庢í鍒囧叧娉ㄧ偣

- [X] T030 [P] 鏇存柊 `specs/010-bundle-windows-poppler/quickstart.md`锛岃褰曞疄闄呮墽琛岃繃鐨勬祴璇曘€丮SI 璺緞鍜屼换浣曟湭鎵ц鍘熷洜
- [X] T031 [P] 妫€鏌?`pinvou3-app/src-tauri/resources/windows/poppler/` 涓槸鍚﹀寘鍚?Poppler 璁稿彲璇佹垨鏉ユ簮璇存槑锛屽繀瑕佹椂琛ュ厖 `pinvou3-app/src-tauri/resources/windows/poppler/README.md`
- [X] T032 杩愯 `cargo check --manifest-path pinvou3-app/src-tauri/Cargo.toml` 骞朵慨澶嶆湰 feature 寮曞叆鐨勭紪璇戦敊璇?- [X] T033 妫€鏌?`git diff --stat`锛岀‘璁ゆ湭淇敼 `DeepSeek-TUI/` 涓斿彉鏇磋寖鍥寸鍚?`specs/010-bundle-windows-poppler/plan.md`
- [ ] T034 鎸?`specs/010-bundle-windows-poppler/contracts/windows-poppler-runtime.md` 鍜?`specs/010-bundle-windows-poppler/contracts/dependency-check-ui.md` 瀹屾垚鏈€缁堥獙鏀惰褰?
---

## 渚濊禆涓庢墽琛岄『搴?
- Phase 1 鏃犱緷璧栥€?- Phase 2 闃诲鎵€鏈夌敤鎴锋晠浜嬶紝蹇呴』鍏堝畬鎴愯祫婧愮洰褰曞拰 OS 灞傛帴鍙ｃ€?- US1 鏄?MVP锛屼緷璧?Phase 2锛屽彲鍏堢嫭绔嬩氦浠?PDF 涓婁紶鍙敤鑳藉姏銆?- US2 渚濊禆 Phase 2锛屼篃鍙湪 US1 瀹屾垚鏍稿績璺緞鍚庢帹杩?MSI 鎵撳寘楠屾敹銆?- US3 渚濊禆 Phase 2锛屽彲涓?US2 鍚庡崐娈靛苟琛岋紝浣嗘渶缁堝簲鍦?US1 閿欒鏂囨绋冲畾鍚庨獙鏀躲€?- Phase 6 渚濊禆 US1銆乁S2銆乁S3 瀹屾垚銆?
## 骞惰鏈轰細

- T003 涓?T004 鍙苟琛岋紝涓€涓獙璇佹簮鐩綍锛屼竴涓爺绌?Tauri 閰嶇疆銆?- T009 涓?T010 鍙苟琛岋紝鍒嗗埆淇敼娴嬭瘯鏂囦欢鍜?quickstart銆?- US1 涓?T011 涓?T012 鍙苟琛岋紝鍒嗗埆瑕嗙洊涓氬姟灞傚拰 OS 灞傛祴璇曘€?- US2 涓?T017 涓?T018 鍙苟琛岋紝鍒嗗埆澶勭悊璧勬簮瀹屾暣鎬ц褰曞拰楠屾敹鍛戒护銆?- US3 涓?T024 涓?T025 鍙苟琛岋紝鍒嗗埆澶勭悊鍚庣娴嬭瘯鍜屽墠绔獙鏀躲€?- Phase 6 涓?T030 涓?T031 鍙苟琛屻€?
## 瀹炴柦绛栫暐

1. 鍏堝畬鎴?Phase 1 鍜?Phase 2锛岀‘淇濊祫婧愪笌 OS 鎶借薄杈圭晫娓呮櫚銆?2. 浠?US1 浣滀负 MVP锛氬厛璁?Windows PDF 涓婁紶浼樺厛浣跨敤鍐呯疆 Poppler銆?3. 鍐嶅畬鎴?US2锛氭妸璧勬簮绾冲叆 Windows MSI 骞堕獙璇佸畨瑁呰惤鐐广€?4. 鏈€鍚庡畬鎴?US3锛氶殣钘?Windows 渚濊禆浣撴涓殑 Poppler 鎵嬪姩琛ュ叏椤广€?5. 姣忎釜鐢ㄦ埛鏁呬簨瀹屾垚鍚庣珛鍗虫墽琛屽搴旂嫭绔嬮獙鏀讹紝涓嶆妸 MSI 鍜?UI 楠岃瘉鍫嗗埌鏈€鍚庛€?
