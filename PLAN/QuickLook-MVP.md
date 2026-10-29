# Explorer Quick Look MVP 開發計畫

- 建立日期：2026-10-30
- 分支：`feature/explorer-quicklook`
- 原始碼評估基準：`6f9f92f`
- 狀態：P1～P4 已實作並分階段提交；P5 已執行開發機原生、自動化及 Linux 回歸驗證，Windows／最低 macOS／Intel 與完整人工驗收仍待完成。詳見 [驗證紀錄](QuickLook-MVP-VALIDATION.md)。
- 目標：依本計畫循序完成可交付的 macOS Quick Look MVP；概念驗證只是第一個里程碑。

## 1. MVP 的使用者體驗與範圍

使用者在 macOS 的 Explorer 選取一個遠端檔案，按空白鍵後，程式下載至私有暫存目錄，再交由原生 Quick Look 面板預覽。使用者不需要選擇下載位置或設定外部編輯器。

### 必須完成

| 項目 | MVP 決策 |
| --- | --- |
| 平台 | 僅 macOS 啟用；Windows／Linux 維持原有操作並可正常編譯。 |
| 來源 | SFTP、SFTP 不可用時的 SCP fallback、S3／相容服務。三者都是交付條件。 |
| 選取 | 僅一個一般檔案；無選取、多選、資料夾、symlink／特殊檔案不啟動下載。 |
| 入口 | 檔案列表有焦點時按空白鍵；右鍵選單提供 `Quick Look` 作為可發現的替代入口。 |
| 關閉／取消 | 下載中再次按空白鍵或 Esc 取消；面板顯示後空白鍵、Esc 或原生關閉按鈕關閉。 |
| 下載提示 | 超過 300 ms 顯示檔名、準備狀態及可取得的位元組進度；完成／取消後移除。 |
| 格式 | 保留原始副檔名，交由系統 Quick Look 與已啟用的擴充處理；不沿用文字編輯器的副檔名過濾。 |
| 大小與時間 | 單檔上限 100 MiB（104,857,600 bytes，含邊界）；從接受請求起 60 秒內須完成下載並準備顯示，否則取消並提示逾時。面板顯示時間不限。 |
| 寫入行為 | 僅下載供檢視，不啟動 editor save watcher、不自動回傳遠端。 |
| 快取 | 不跨次重用下載結果，每次開啟重新取得檔案；只保留目前預覽所需的暫存資料。 |

### 延後項目

- 多選預覽、資料夾遞迴下載與預覽、symlink 跟隨。
- 面板開啟時以方向鍵自動下載並預覽相鄰檔案。
- 持久快取、背景預抓、ETag／mtime 快取失效機制。
- 大檔案串流播放、只下載部分內容就開始預覽。
- Quick Look 擴充開發、自製各種格式的 renderer、非 macOS 的替代預覽器。
- 在預覽中編輯、Markup 修改回傳、Open With 工作流程整合，以及可調整大小上限的設定頁。

第一次預覽仍須等待完整下載。系統不支援的格式可以呈現 Quick Look 的圖示／資訊等預設結果；不承諾每種格式都有內容預覽。這是呼叫系統預覽的功能，不是新增格式支援。

## 2. 原始碼依據與接點

以下路徑相對於 repository root。行號會隨實作變動，請以符號名稱定位。

| 位置 | 已確認現況 | 實作方式 |
| --- | --- | --- |
| `src/components/explorer/ExplorerFileTable.tsx` | 有 `selectedIds`、`selectedEntries`、row `onKeyDown`、右鍵選單。 | 新增可選的 `onQuickLook` callback；以平台能力與選取數決定入口。 |
| `src/components/sftp/ExplorerPage.tsx` | Explorer 分流 SFTP／SCP 與 S3，傳遞 `isActive`。 | 延續目前分流；接入隱藏／關閉時的預覽釋放。 |
| `src/components/sftp/ExplorerView.tsx` | 使用 `transport` 區分 SFTP／SCP。 | 將實際 transport、session 與 entry 傳給預覽控制器。 |
| `src/components/s3/S3Browser.tsx` | 有獨立 session 與 bucket 切換。 | 預覽目標包含 bucket；切換 bucket 時取消舊請求。 |
| `src/lib/explorer-transport.ts` | 集中轉換 SFTP／SCP command 名稱。 | 可參考分流方式，但不必為預覽新增三套相同 IPC。 |
| `src/types/explorer.ts`、`src/providers/*` | `FileSystemProvider.type` 只有 `sftp`／`s3`；SCP 共用 SFTP provider。 | 預覽請求另外定義三種 transport，不能只靠 `provider.type` 判斷 SCP。 |
| `src-tauri/src/sftp/commands.rs` | `stage_file` 可串流落盤；`sftp_drag_out` 有私有暫存與原生主執行緒派送範例。 | 抽取最小必要下載 helper，補取消、限額與進度。不要直接呼叫 drag-out command。 |
| `src-tauri/src/scp/transfer.rs` | `download_file` 已有 cancellation token 與進度 callback。 | 延用傳輸協定處理，補 header 大小驗證及停滯／取消路徑。 |
| `src-tauri/src/s3/transfer_manager.rs` | `run_download_file` 已有串流／分段下載、取消與進度範例。 | 抽用必要下載邏輯；綁定請求當下的 bucket handle。 |
| 各協定的 `*_edit_external`、`src-tauri/src/editors/mod.rs` | 有暫存與下載，但編輯流程會監看存檔並回傳；SFTP／S3 編輯流程會完整緩衝內容。 | 只參考路徑隔離概念；預覽不呼叫編輯命令，也不使用整檔記憶體緩衝。 |
| `src-tauri/src/lib.rs`、`src-tauri/Cargo.toml` | Tauri command 註冊與狀態管理入口；目前未整合 Quick Look。 | 註冊預覽 manager、commands、生命週期 hooks 與 macOS 專用依賴。 |
| `.github/workflows/ci.yml`、`tests/e2e/README.md` | 主要測試與 E2E 在 Linux；無法證明 macOS 原生面板可用。 | 保留 Linux 回歸，增加 macOS 編譯檢查並執行原生手動驗收。 |

## 3. 技術方案

### 3.1 原生面板

- 正式實作用 `QuickLookUI.QLPreviewPanel`，透過 Rust 的 `objc2` 系列 bindings 呼叫 AppKit／Foundation／QuickLookUI。
- 在 Cargo 的 macOS target dependencies 下明確宣告所需 bindings；依現有 lockfile 相容性選版本及 features，不依賴間接依賴可直接使用的假設。
- 在主執行緒建立與操作原生物件；以受控的 `NSResponder`／controller 加入既有 responder chain，實作 `acceptsPreviewPanelControl`、`beginPreviewPanelControl`、`endPreviewPanelControl`。
- controller 取得控制權後才設定 data source／delegate；正確保留物件生命週期，失去控制權時解除引用。不要取代 Tauri 的 window delegate 或全域攔截所有視窗。
- 使用 data source 提供單一暫存檔案 URL，delegate 處理必要的按鍵與關閉回呼；驗證空白鍵不會被影片播放控制吃掉。
- 面板即使取得焦點，關閉與回復 Explorer row 焦點也須可用。不能只在 React 的 `keydown` 假設會收到所有按鍵。
- `qlmanage -p` 可用於開發時的人工診斷，但不作為正式交付替代方案。

第一個里程碑必須先驗證此橋接方式。若 Tauri responder chain 與面板控制有問題，先在這一層修正並更新計畫，不讓後續下載功能掩蓋原生整合未完成的事實。

### 3.2 建議模組

```text
src/lib/quick-look.ts                          # IPC 型別、平台能力、請求事件
src/hooks/use-quick-look.ts                    # Explorer 行為、進度、失效處理
src/components/explorer/ExplorerFileTable.tsx  # 空白鍵與右鍵入口
src-tauri/src/quicklook/mod.rs                 # 型別、manager、狀態／限額
src-tauri/src/quicklook/commands.rs            # capabilities / open / close
src-tauri/src/quicklook/staging.rs             # 協定分流、下載、私有暫存與清理
src-tauri/src/quicklook/macos.rs               # cfg(macOS) 原生橋接
```

小範圍調整現有傳輸 helper 以供重用即可，不藉此改寫整個傳輸佇列。預覽使用獨立的短生命週期下載任務與進度事件，不混入一般下載歷史或對外編輯工作流程。

### 3.3 IPC 契約與狀態

建議 commands：

- `quicklook_capabilities() -> { supported, maxBytes }`：由後端回報平台能力；非 macOS 回傳 `supported: false`。
- `quicklook_open({ requestId, revision, ownerId, target }) -> accepted`：驗證與登錄請求後立即回傳，後續結果透過事件傳送。
- `quicklook_close({ revision, ownerId, requestId }) -> ()`：取消匹配請求或關閉匹配面板；重複呼叫安全。

`target` 為具標籤的 union：

```ts
type QuickLookTarget =
  | { transport: "sftp" | "scp"; sessionId: string; path: string }
  | { transport: "s3"; sessionId: string; bucketName: string; key: string };
```

- 原生 window 從 Tauri command context 取得；前端不能指定別的 window 或任意本機路徑。
- `ownerId` 識別 Explorer 實例，`requestId` 識別單次開啟；同一個 window 的控制器共用遞增 `revision`，open／close 都攜帶它。
- manager 對每個 window 序列化狀態變更，拒絕過期 revision。close 先抵達時也更新 revision，讓稍後才抵達的舊 open 無法重新啟動。
- Quick Look shared panel 是程序共用單例；原生層保留目前 request 身分，新視窗取得面板前通知舊 owner，舊 owner 的延遲關閉不能影響新面板。
- 同一個 window 同時最多一個有效預覽。新 open 使舊任務失效、取消並關閉舊面板；新下載等舊 writer 釋放後才開始，避免暫存與下載量無界增加。
- 舊 owner 的延遲清理不能關掉新 owner 的面板；關閉需檢查 owner／request 身分，不以「關閉所有預覽」實作前端 cleanup。
- 新增 `quicklook:state` 事件：`requestId`、`ownerId`、`revision`、`phase`、`bytesReceived`、可選 `totalBytes`／`error`；只發送到所屬 window。
- `phase` 至少涵蓋 `preparing`、`downloading`、`visible`、`closed`、`cancelled`、`error`。開啟前建立 listener，舊事件不更新新 UI；必要時提供狀態快照解決 listener 重建問題。
- 錯誤沿用 `{ kind, message }` 格式，至少區分不支援平台、無效選取、大小超限、逾時、斷線／權限、下載失敗、原生面板失敗。使用者取消不顯示失敗 toast。

狀態流向：

```text
idle → preparing → downloading → visible → closed → idle
          └──────────┴──────────→ cancelled / error → idle
```

每次下載完成、準備交接主執行緒及實際顯示前，都重新確認 request 仍有效。禁止下載 A 較慢完成後，覆蓋較新的 B，或在使用者取消後突然彈出面板。

### 3.4 下載、驗證與暫存

1. 後端驗證 session，解析檔案身分與類型；不信任列表傳來的 size、entryType 或檔名。
2. SFTP 檢查 symlink／一般檔案；SCP 檢查 metadata 並驗證 wire file header 的大小；S3 取得 object metadata，確認請求 bucket 與目前 session 一致後固定 bucket handle。
3. 預先知道超過 100 MiB 時不下載。傳輸中仍強制計算實收 bytes，超額立即中止；不可只靠預檢，亦不可因未知大小而取消上限。
4. SCP 如需擴充 helper，讓既有呼叫端可維持原行為，預覽額外傳入限制。S3 使用單次串流 GET 避免分段版本混用，要求 strong ETag 並送出 If-Match；下載後再檢查 metadata。缺少 validator 或物件改變時回報失敗，不顯示不完整檔案。
5. 以固定大小 buffer 串流到私有目錄中的 `.part`；保持 UI 與 metadata 查詢可用，不在完整傳輸期間持有 session manager 全域鎖。
6. cancel／timeout 必須能中斷等待中的網路 I/O，而不只檢查區塊之間的旗標。終止預覽專用 channel／task，不拆掉其他 terminal 或一般傳輸的共享 SSH 連線。
7. 檔案完整接收、flush 並關閉 writer 後，才 rename 為保留原副檔名的最終名稱，交給 Quick Look。失敗或取消絕不預覽 `.part`。

暫存策略：

- 使用程式專用 root，再依 process/run UUID 與 request UUID 隔離，禁止同名檔案互相覆蓋。
- Unix 目錄權限 0700、檔案 0600；清理只涵蓋後端建立且追蹤的目錄，不接受前端指定清理路徑。
- 驗證 basename：排除 `.`、`..`、分隔符、NUL 等不安全名稱；保留正常 Unicode、空格與副檔名。S3 key 可含 `/`，只取安全 basename，不把完整 key 拼入本機路徑。
- 面板讀取期間保留檔案。關閉時先解除 data source／原生引用，再安排刪除；檔案仍被使用時延遲重試。
- 取消與失敗需等 writer 結束後移除部分檔案。程式關閉時取消任務並清除本次 run；異常結束留下的目錄在啟動時掃描清理。
- stale sweep 只處理可確認非存活 run 的程式專用目錄，使用 run ownership／鎖避免多程序互刪。清理失敗記錄可診斷訊息並在下次重試。
- 不寫入檔案內容、憑證或 presigned URL 到日誌／telemetry。程式只保證自身 staging 清理，不宣稱能控制系統 Quick Look 的所有快取。

## 4. 前端互動與生命週期

- 在共用 table 的 row／table 範圍處理空白鍵，阻止頁面捲動；不新增未限定作用範圍的全域空白鍵快捷鍵。
- 只有可見且 active 的 Explorer、單一有效檔案選取才觸發。忽略 `event.repeat`、IME composition、任何修飾鍵與 input／textarea／contenteditable 中的輸入。
- 右鍵入口沿用選取規則：點擊未選取的檔案時先選取它；多選狀態不提供可用的單檔預覽。進行中按相同檔案的空白鍵／右鍵入口採關閉／取消行為。
- 下載中保留檔案列表焦點，Esc 可取消；提示在 300 ms 後出現，已知 total 時顯示 bytes 進度，未知 total 顯示不定進度。事件節流至約 100 ms，避免高頻重繪。
- 面板可見時，必要按鍵交由原生層處理。關閉後只在原 Explorer 仍 active、row 仍存在時還原焦點，不強行切回已離開的分頁。
- 改變選取時取消／關閉舊預覽；新選取需再次按空白鍵才預覽。變更目錄、切換 bucket／tab、關閉 tab、session 中斷及元件卸載同樣釋放舊請求。
- 前端失效通知與後端 session／window 關閉 hooks 都要處理；即使前端未及時通知，舊下載也不能在已關閉視窗上顯示。
- 非 macOS 不顯示 Quick Look 選單、不吞掉空白鍵。平台差異透過後端能力回報與 `cfg` 隔離，不引入原生符號到其他平台的編譯路徑。

## 5. 依序執行的工作包

| 順序 | 工作與產出 | 完成條件 |
| --- | --- | --- |
| P1 | 建立 `macos.rs` 與必要 target dependencies，以開發用本機 fixture 驗證原生橋接。fixture 入口不得進入正式 IPC。 | 真實 Tauri 視窗可開／關面板、取得控制權、處理空白鍵／Esc、還原焦點；重複開關 20 次無 crash／失控面板。 |
| P2 | 建立 manager、IPC、事件與請求版本管理；接上私有 staging 及 SFTP 串流下載。 | SFTP 圖片／PDF／文字可預覽；取消、超限、逾時與過期完成不能彈窗。 |
| P3 | 加入 SCP 與 S3 adapter；補 header／實收大小限制、bucket 快照及取消。 | 三種來源都通過成功與失敗路徑；SCP 同時驗證 GNU／busybox server；S3 切換 bucket 不串檔。 |
| P4 | 接上 table callback、右鍵選單、進度、active owner 與 navigation／tab／session lifecycle。 | 從 Explorer 實際完成空白鍵開關，不干擾文字輸入、terminal、重新命名及既有雙擊設定。 |
| P5 | 完成自動測試、macOS 編譯檢查與原生手動驗收，更新 README 與驗證紀錄。 | 第 6、7 節全部有結果；移除開發入口，提交功能與驗證證據。 |

預估熟悉 Rust／Tauri 與 AppKit 的單人投入約 4～8 個工作天，包含三種來源與基本驗證。P1 是主要技術風險；若 responder chain 或最低 macOS SDK 相容性不如預期，依實測修正估時。不得把 P1 完成等同整個 MVP 完成。

建議後續 commit 依工作包切分：原生橋接、下載 manager 與 SFTP、SCP／S3、Explorer 接線、驗收與文件。每次 commit 應可編譯，未完成入口先不向使用者曝光。

## 6. 測試與驗證方式

### 自動測試

- React／Vitest：單檔觸發、空白鍵 toggle、repeat／composition／修飾鍵不觸發、輸入中不觸發、多選／資料夾／symlink 不觸發、非 macOS 不吞鍵。
- 控制器：A→B 亂序完成、close 先於舊 open、舊 owner cleanup、重複事件、listener 準備前後競態、分頁與 bucket 切換、取消不報錯、焦點還原條件。
- Rust：100 MiB 邊界及實收超限、偽造 metadata、未知大小、取消卡住的 reader、60 秒 deadline、`.part` 不可顯示、失敗後清理、basename 驗證與同名隔離。
- 原生層以小型抽象隔離，狀態測試可用假 panel；假 panel 測試不代替真實 macOS 面板驗收。
- 傳輸 adapter 使用可控測試資料與測試服務驗證三種來源；覆蓋權限拒絕、檔案刪除、斷線、空檔案及 S3 下載中物件變更。
- 不為文件或 getter 撰寫形式測試；優先覆蓋取消、競態、資料隔離與跨平台退化等有風險的行為。

### 建置與回歸命令

先在 repository root 執行前端命令，確保產生 `dist/`，再進入 `src-tauri/` 執行 Cargo 命令：

```sh
pnpm build
pnpm test
```

```sh
cd src-tauri
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- macOS 執行上述檢查及 `pnpm tauri build --debug`，確認真實 app 啟動與 QuickLookUI framework 連結。
- CI 增加 macOS 編譯／測試 job；Linux 延用現有 CI 回歸；Windows 至少執行原生平台編譯檢查。不能只在 Mac 指定 Linux／Windows target 就宣稱已完成平台驗證。
- 現有 Linux `make e2e` 可驗證既有 Explorer 回歸；依實際改動選擇相關 SFTP／SCP／S3、雙擊、重命名與 tab lifecycle specs。原生預覽由 macOS 手動驗收補足。
- 若執行環境缺少 pnpm、Rust、SDK 或必要依賴，先列明缺項並依專案使用者指示取得安裝核准；未執行的檢查明確記為未驗證。

### macOS 手動驗收矩陣

每項記錄 OS／架構、app commit、來源、fixture 大小、步驟與結果；可附截圖或錄影，但不提交遠端真實資料或憑證。

| 情境 | 預期結果 |
| --- | --- |
| SFTP、SCP、S3 各預覽 PNG、PDF、TXT | 完整下載後顯示原生內容；不用外部編輯器或選擇儲存路徑。 |
| 小型 MP4／音訊 | 系統支援時可播放；空白鍵仍遵守本功能的關閉規則。 |
| 已安裝並啟用的第三方預覽擴充 | 選一個只有該擴充提供的格式實測；記錄 OS／擴充版本，不宣稱所有擴充皆相容。 |
| 未支援副檔名／空檔案／損壞檔案 | 系統顯示 fallback 或可理解的失敗；app 不崩潰、不啟動外部 app。 |
| 慢速下載中按空白鍵／Esc、按住空白鍵 | 單次請求可取消；不重複下載，完成後不再彈窗。 |
| 100 MiB、超過上限、metadata 低報大小、卡住 60 秒 | 邊界符合規格；超限／逾時不顯示部分內容，清理殘檔。 |
| A 未完成就切 B 並預覽 B | 只顯示 B；舊完成／事件／清理不能影響 B。 |
| 切目錄／bucket／tab、關閉 tab、斷線 | 舊請求取消或面板關閉；無失效預覽彈出。 |
| Unicode／空格／同名不同主機及 bucket | 名稱與格式正常，內容不混用。 |
| 面板開關 20 次、失去 app 焦點後返回 | 無 crash／殘留面板，關閉後焦點回到仍有效的 row。 |
| 取消／關閉／異常結束後重新啟動 | staging 按策略清理，不刪除其他存活 run 的檔案。 |
| Terminal 空白鍵、rename、新增檔案、路徑輸入、既有雙擊 | 原有操作正常；未啟用預覽時沒有快捷鍵副作用。 |

原生驗收至少在開發機執行並記錄實際 OS／架構。專案 README 宣稱 macOS 11+，因此新增 bindings／API 必須檢查最低版本與可用性；未測最低版本或 Intel 時明列未驗證範圍，不能沿用既有宣稱作為證據。

## 7. Definition of Done

- [ ] P1～P5 全部完成，三種遠端來源都能從 Explorer 單檔預覽。
- [ ] 空白鍵、Esc、原生關閉與焦點回復符合第 1、4 節。
- [x] 下載中的提示、大小上限、deadline 與實際 I/O 取消均已驗證（自動化／真實 adapter；牆鐘與完整人工流程的範圍見驗證紀錄）。
- [ ] A／B 競態、過期 open／close、tab／bucket／session 失效都有自動測試及相關手動結果。
- [x] staging 隔離與清理完成；預覽不走 editor watcher，不回傳遠端。
- [ ] 非 macOS 的 UI 退化、Linux 回歸與 Windows 編譯結果已記錄。
- [ ] macOS debug bundle 成功建置，真實原生面板驗收完成；不能只以 mocked IPC 宣告成功。
- [x] README 說明操作、首次下載、100 MiB／60 秒限制、系統格式支援與 macOS 限定。
- [x] 在 `PLAN/QuickLook-MVP-VALIDATION.md` 記錄實測 commit、環境、指令結果、驗收矩陣及未驗證限制；只在實際執行後建立結果，不預填通過。
- [x] 無臨時 fixture IPC 或真實遠端資料／憑證進入正式介面／提交；合成資料僅限測試碼。

## 8. 官方參考與待確認事項

- [Apple QLPreviewPanel](https://developer.apple.com/documentation/quicklookui/qlpreviewpanel)：shared panel、responder chain 與控制權生命週期。
- [Apple QLPreviewPanelDelegate](https://developer.apple.com/documentation/quicklookui/qlpreviewpaneldelegate)：面板事件與 window delegate 行為。
- [Apple previewItemURL](https://developer.apple.com/documentation/quicklook/qlpreviewitem/previewitemurl)：預覽項目使用 file URL。
- [Tauri WebviewWindow](https://docs.rs/tauri/latest/x86_64-apple-darwin/tauri/webview/struct.WebviewWindow.html)：`ns_window`、`run_on_main_thread`。
- [objc2 QuickLookUI bindings](https://docs.rs/objc2-quick-look-ui/latest/objc2_quick_look_ui/struct.QLPreviewPanel.html)：Rust 原生橋接候選；實作時以選定版本的文件與 SDK 為準。

待 P1 實測確認：Tauri responder chain 安裝方式、媒體預覽按鍵行為、最低 macOS 可用性及 bindings 版本。待 P3 確認：現有 S3 library 的條件下載 API、SCP header 限額擴充方式。這些都有指定工作包與驗收，不代表可省略對應功能。
