# Quick Look MVP 實作與驗證紀錄

- 日期：2026-10-30（Asia/Taipei）
- 分支：`feature/explorer-quicklook`
- 起點：`74b95e4`；最後原始碼 commit：`81006c3`。本紀錄另立文件 commit。
- 狀態：**P1～P4 實作完成，P5 已完成本機可執行的自動／原生驗證；完整人工驗收、Windows、最低 macOS 與 Intel 尚未完成。不能視為整份 Definition of Done 已全部通過。**

後續 [Code Review 核對紀錄](QuickLook-CODE-REVIEW-2026-10-30.md) 已發現 6 項待處理問題（包含一項已用隔離 Rust 程式重現的取消競態）。本文件保留 `80b89d4` 交付時的驗證結果；不代表後續審查問題已修正。

## 分階段 commit

| Commit | 產出 |
| --- | --- |
| `367611f` | P1：objc2／QuickLookUI 橋接、受控 responder、面板專用鍵盤 monitor、非正式 IPC 的原生測試 example。 |
| `0103c24` | P2：window／owner／revision 管理、IPC、事件、私有 staging、SFTP 獨立 channel、大小與期限。 |
| `f7eb0ba` | P3：SCP header／實收限制、S3 bucket 快照與條件串流 GET、明確關閉預覽 channel、合成測試服務。 |
| `cb3c5ad` | P4：Explorer 空白鍵與選單、300 ms 進度、取消、選取／導航／分頁失效、焦點回復條件。 |
| `9067689` | P5：補關機競態、銷毀視窗墓碑、strong ETag、原生跨視窗控制權與對應測試。 |
| `5c939a5` | P5：卡住的網路 I/O、重複取消、bucket 同名 key 隔離、If-Match 實測、原生 CI。 |
| `229a484` | 等待 Tokio 背景磁碟寫入完成後才刪除 staging／釋放 writer gate。 |
| `714760d` | 原生失敗路徑及 process exit 先解除 delegate／data source 與 responder。 |
| `19c48ab` | 按住空白鍵／Esc 不重複操作，也不讓預覽中的列表因空白鍵 repeat 捲動。 |
| `4e6cb64` | 接管延遲確認的 SSH channel open、限制待確認工作、取消 SCP metadata，並移除已認證 handle 上不必要的全域 mutex。 |
| `4d0ee85` | 初次解析家目錄時先標示 loading，防止重新整理搶先載入根目錄；加入延遲解析測試。 |
| `3e3a7b0` | E2E helper 等待 Explorer 初次載入完成，避免測試操作初始路徑占位值。 |
| `81006c3` | SSH outgoing mailbox 背壓時保留待 close 的 channel 所有權，本機檔案清理保持有界。 |

## 實作重點

- 正式 IPC 只有能力、遠端目標 open、身分匹配 close 與狀態快照；不接受本機預覽路徑、清理路徑或其他 window label。
- 每個 window 使用遞增 revision 與單一 writer gate；原生共享面板另檢查 request 身分。舊下載、清理、事件不能覆蓋新請求。銷毀視窗保留永久墓碑；關機與登錄共用鎖，且保留原 exit／restart code。
- 預覽走獨立 channel／短生命週期工作，不加入下載歷史、不呼叫 editor watcher、不自動回傳遠端。取消可中斷等待中的讀取，並明確關閉 channel；取消後仍在 Tokio blocking pool 的本機寫入會先被等待、釋放。
- russh 0.46 的 channel-open future 不可在收到確認前直接丟棄；以每連線一個 preview permit 接管確認與未領取 channel，且不持有共用 handle mutex、不阻塞正常操作或 disconnect。SCP stat／POSIX fallback 同樣走可取消 scope，metadata 輸出上限 64 KiB。SSH close 若遇 outgoing mailbox 背壓，保留 channel 所有權直到 close 排入或連線結束；本機檔案清理不必無限等待網路。
- 100 MiB 邊界包含在內；metadata 與實收 bytes 都檢查。60 秒 deadline 包含等候前一個 writer、下載與準備面板，面板顯示時間不限。
- S3 固定接受請求時的 bucket clone，要求 strong ETag，送出帶引號的 If-Match，以單次串流 GET 下載，再檢查 metadata。物件改變、缺少 validator、超限、權限拒絕或不完整資料都不發布完成檔。
- staging 以 run／request UUID 隔離；Unix 目錄 0700、檔案 0600。先 flush、關閉 writer，再 rename。解除原生引用後才清理；暫時失敗會重試，再由下次啟動 sweep 接手。owner lock 防止多程序互刪。
- 原生測試 example 保留於 `src-tauri/examples/quicklook_probe.rs`，只作可重跑的測試程式；不編入正式 app 入口，也沒有 fixture IPC。合成 PDF／公開 fixture 密碼只存在測試碼／測試服務，沒有真實遠端資料或憑證。

## 環境

| 項目 | 實際環境 |
| --- | --- |
| macOS | 27.2，build 26B123，arm64 |
| SDK／部署版本 | macOS SDK 27.0；`vtool -show-build` 顯示 app `minos 11.0`。這不是 macOS 11 執行測試。 |
| Mac 工具 | Rust 1.99.1；Node 26.0.0；pnpm 11.25.0 |
| 原生依賴 | objc2 0.6.4；objc2-app-kit／foundation／quick-look-ui 0.3.2；macOS target 專用宣告。 |
| Linux | Docker 中 Ubuntu 24.04.5 LTS，aarch64，Rust 1.99.1，Node 22.23.4，pnpm 12.16.1，WebKitGTK／Xvfb。不是在 Mac 只指定 Linux target。 |
| 遠端 fixtures | loopback Docker：SFTP、GNU SCP-only、busybox SCP-only、MinIO；另以受控 HTTP server 測錯誤與停滯。 |

## 已執行的檢查

| 版本／指令 | 結果 |
| --- | --- |
| `4d0ee85`：`pnpm build` | 通過 TypeScript 與 Vite build。既有動態匯入／bundle size 警告仍存在。 |
| `4d0ee85`：`pnpm test` | 34 個檔案、201 項通過。涵蓋單檔／無選取／多選／資料夾／symlink／特殊檔案、輸入／IME／修飾鍵／repeat、平台退化、選單選取、延遲進度、owner 與焦點條件，以及初次家目錄解析的 loading 競態。 |
| `81006c3`：macOS fmt／Clippy／cargo test | 通過；229 項 Rust 測試通過，5 項需服務的整合測試在一般 test 命令中明確 ignored，並另外執行如下。 |
| `81006c3`：`tests/quicklook/run-adapters.sh` | 5 項實際協定測試全部通過。 |
| `81006c3`：Linux 原生 fmt／Clippy／cargo test | 全部通過；229 項通過、5 項 fixture 整合測試 ignored。 |
| `3e3a7b0`：Linux 選定 E2E | 21 個 spec、33 個案例全部通過：SSH key／重新連線、SFTP 列表／導航／重新命名／下載、S3 列表／下載、GNU／busybox／無 stat 的 busybox SCP、sudo 切換、symlink／broken symlink、分頁切換／關閉、Terminal split、port forwarding／ProxyJump 與多跳、雙擊大檔進度及 Quick Look 平台退化。 |
| `714760d`：原生 TXT probe | 真實 Tauri 視窗 20 次循環通過；包含取得控制權、Space、Esc、原生關閉、另一個 window 成為 main 後釋放，以及面板隱藏與清理。 |
| `9067689` 原生橋接：七種 fixture probe | PNG、有效 PDF、MP4、WAV、空 TXT、損壞 PDF、未知格式，各 20 次，140 次全部通過。後續 `714760d` 增加失敗／退出清理後，另重跑上述 TXT 20 次。**這些是面板／item／事件驗證，沒有把 renderer 內容目視驗收標為通過。** |
| `714760d`：隔離 identifier 的真實 debug app | 啟動兩次，均建立資料庫、私有 staging 並保持程序正常；第一次 SIGKILL 後第二次啟動移除舊 run，只留下新 run。隔離資料、cache 與測試 bundle 已移除。未使用既有連線資料庫。 |
| `81006c3`：正常 identifier 的最終 debug app bundle | 本機停用 updater artifacts 的設定建置成功，產生 `src-tauri/target/debug/bundle/macos/anySCP.app`。 |

擴大 E2E 的第一次執行為 32／33；失敗時的 HTML 顯示遠端路徑仍為 `/`，定位到家目錄解析完成前 refresh 的競態。已修正實際 Explorer loading 狀態、加入延遲 home regression，並讓 helper 等候初次載入完成；重跑為上述 33／33，沒有放大 timeout 掩蓋問題。

協定整合測試實際覆蓋：

- 三種來源的 PNG／有效 PDF／TXT／空檔案與內容相等檢查；Unicode、空格及同名檔案隔離。
- SFTP／GNU SCP／busybox SCP 拒絕 symlink、資料夾、缺檔、權限拒絕與超過 100 MiB 的檔案。
- 同一 SSH handle 連續取消 12 次，再完整下載成功；SFTP／GNU SCP／busybox SCP 各執行一次。
- 真正卡住的 SFTP／SCP reader 及 S3 HTTP body 可取消；不等遠端 EOF，之後共享 SSH 仍可執行命令。
- S3 切換到另一 bucket 後，舊 bucket snapshot 與新 bucket 的同名 key 分別取得正確內容。
- MinIO 實際拒絕舊 If-Match；私有 clone 的 header 不污染共享 bucket。
- 受控 HTTP：無／weak ETag、未知大小、超限 metadata、物件變更、403／412、停滯取消。不把部分檔案交給 Quick Look。

229 項一般 Rust 測試包含延遲 SSH open 確認後的回收、重複取消時限制待確認工作、不阻塞共用 disconnect、取消 GNU stat／POSIX metadata fallback、metadata flood 上限，以及 100 MiB 含邊界／實收超限／偽造 metadata、取消卡住 reader、虛擬時間的完整 60 秒 deadline、close 先於 open、舊 owner 清理、銷毀視窗墓碑、關機、`.part`、權限與 live-run sweep 等。60 秒邏輯以 Tokio paused clock 驗證；沒有宣稱對每個協定都等待真實 60 秒做網路停滯測試。

## 原生／人工驗收矩陣

以下「自動驗證」與「人工未驗證」分開記錄，避免將假的 panel 或 IPC 測試當成真實原生成功。

| 計畫情境 | 已有證據 | 尚未驗證 |
| --- | --- | --- |
| 三種來源 PNG／PDF／TXT | 實際協定下載與內容比對；獨立真實 Quick Look 面板／URL probe。 | 從 macOS Explorer 操作到各來源的完整人工目視流程。 |
| MP4／音訊 | 真實面板各 20 次；Space／Esc 關閉事件通過。MP4 2,276 bytes；WAV 16,044 bytes。 | 人工確認播放內容與播放控制互動。 |
| 第三方預覽擴充 | 接口交給系統 Quick Look，未限定文字副檔名。 | 指定擴充與版本、實際 renderer 使用情形。 |
| 未支援／空檔／損壞檔 | 33-byte 未知格式、0-byte TXT、24-byte 損壞 PDF，原生面板各 20 次，不崩潰。 | fallback 畫面的目視確認。 |
| 慢速下載、取消與按住空白鍵 | React、控制器、卡住的真實 reader、重複取消均通過；repeat 不觸發也不捲動有效選取列表。 | Mac Explorer 長按與拖延網路的完整目視流程。 |
| 上限與期限 | Rust 實收 100 MiB／+1、低報／未知大小；實際服務超限預檢；虛擬時間 60 秒。 | 每個來源的實際 100 MiB 面板與 60 秒牆鐘手動測試。 |
| A→B、close／owner／listener 競態 | TS／Rust 自動測試通過；單一 writer gate 等待舊 writer／channel／磁碟工作釋放。 | 跨元件／視窗的人工快速連續操作。 |
| 目錄／bucket／tab／session | Hook 失效測試、實際 bucket 隔離；後端 session／SSH status／window hooks；Linux tab 回歸。 | macOS 斷線／分頁切換的完整原生 UI 驗收。 |
| Unicode／空格／同名 | 真實來源內容比對；原生「測試 image.png」1,753 bytes／「測試 document.pdf」596 bytes 通過。 | 同名不同真實主機的人工目視流程。 |
| 反覆開關／控制權／焦點 | 最終 TXT 20 次及七種格式 140 次；新增另一視窗切換測試；React 只在仍 active、原 row 存在且目前 window 有焦點時還原。 | app 失焦後返回、實際 Explorer row 焦點及最低 OS 行為。 |
| staging 清理與重啟 | 權限／UUID／live-run lock／失敗清理／背景 writer 測試；真實 app SIGKILL 後重啟 sweep 通過。 | 系統 Quick Look 自身快取不屬 app staging 保證。 |
| Terminal／rename／雙擊回歸 | 前端全套；Linux sudo／symlink／rename／大檔 editor 雙擊回歸。 | Mac Terminal 與其他原生視窗的完整人工鍵盤驗收。 |

## CI、打包與限制

- CI 保留原有 Linux 回歸，新增 macOS／Windows 原生編譯 job、macOS Clippy／tests／app bundle，以及 Linux 實際 adapter fixture 測試。
- **沒有推送分支、建立 PR 或觸發遠端 workflow。新增 CI job 尚未在 GitHub 執行；Windows 結果為未驗證。**
- 最低 macOS 11、Intel、其他 macOS 版本未執行。以舊版可用的 QLPreviewPanel／NSResponder API 與 `minos 11.0` 建置，不等於已在那些系統驗收。
- 預設 `pnpm tauri build --debug` 已實際執行，但 DMG 腳本失敗；單獨預設 app bundle 產生後也因缺少 release updater 私鑰而 exit 1。沒有取得或生成 release 私鑰。
- 本機可用的 debug 設定是 `pnpm tauri build --debug --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'`，不修改正式打包設定。正式 DMG、release signing／notarization 留待既有發布流程處理。
- Linux VNC 測試以本機臨時 Compose override 取消 host port 發布。合成測試服務、隔離 app 的資料均在測後清除；Docker／Cargo／套件快取保留供後續使用。

## Claude Code 諮詢紀錄

**本工作階段共呼叫 3 次**，皆為非互動式 `--model opus --effort high`；第二、三次使用 `--continue` 延續同一對話的專案脈絡。沒有請 Claude 修改檔案或啟動其他 agent。

| 次數 | 問題 | 回覆重點與採用結果 |
| --- | --- | --- |
| 1 | P1 如何選 objc2 版本／features、插入 Tauri NSWindow responder chain、管理 data source／delegate、處理 Space／Esc／關閉與 main-thread 生命週期？ | 建議匹配 objc2 0.6／framework 0.3，明確宣告 target 依賴；三個控制權方法是 NSObject category，使用自訂 responder，不換 window delegate；保留 unretained 物件、面板限定的 event monitor，並注意程序共享單例。已以 crate 原始碼、編譯與真實面板循環驗證後採用。實測另修正先顯示面板再檢查 AppKit 控制權的順序。 |
| 2 | 目前實作取消／清理有何具體風險？russh stream drop、russh-sftp 背景工作、session／window 競態，以及單次 S3 GET＋If-Match＋HEAD 是否足夠？ | 確認 ChannelScope 顯式 close 方向，要求同 handle 多次取消後再成功的實測；指出缺 ETag、If-Match 未實證、關機登錄競態、銷毀視窗無墓碑、控制權喪失殘留面板、Drop 中 spawn 等風險。已補條件 GET／bucket 隔離、12 次取消與真實停滯 reader、關機鎖與有界等待、墓碑、同步排隊清理與 owner-window notification。最後另自行補 Tokio pending disk-write 等待與失敗／退出時的原生解除引用。 |
| 3 | 取消 SSH channel open 後，延遲的 Open confirmation 是否會洩漏 channel？SCP metadata 尚未進入 relay 前應如何取消？既有共用 handle mutex 會否阻塞 disconnect？ | 建議只接管不可直接丟棄的 open handshake，不讓 scope 等候無期限確認；指出持有 handle mutex 等確認會鎖住其他操作；metadata 應接受 optional scope，並用真實 SSH peer 測延遲確認、停滯與輸出上限。檢查本機 russh 原始碼後採用 immutable SharedHandle、每連線一個 preview permit、可明確領取的 shared slot，並加入四項真實 loopback SSH 測試。沒有照抄不符目前 crate 的建議 API。 |

未直接採納移除最後一次 S3 HEAD 的建議：保留較保守的 metadata 驗證，以偵測不完整／改變中的物件；代價是物件在準備期間更新時，可能需要重試。沒有以顧問說明代替實測。

## 後續人工收尾

1. 依上表在 macOS Explorer 逐項操作，特別是實際 row 焦點、第三方擴充與媒體內容。
2. 在 Windows 原生執行新 CI；有可用環境時補 macOS 11／Intel。
3. 需要正式發布時，再處理 DMG 與原有 release 簽章流程。

重跑命令與合成 fixture 說明見 [tests/quicklook/README.md](../tests/quicklook/README.md)。

## 2026-10-30：空白鍵關閉閃爍修正

- 人工回報再次按 Space 時，面板閃爍後仍保持開啟。擴充原生 probe，送出 KeyDown／KeyUp，並在原生關閉 callback 後立即從背景執行緒派送 `macos::close`，以符合正式 manager 的清理時序。原有 probe 延後到可見性斷言後才清理，沒有覆蓋此情境。
- 修正前以此時序重現 `closed/control-loss panel must be hidden` 失敗：`panel.close()` 後面板仍在關閉動畫中，cleanup 再次 `orderOut()` 會干擾該轉場。控制權轉移的路徑也有重複 order-out。
- 原生 controller 新增 closing 狀態，使關閉／隱藏只啟動一次；Space／Esc 等待 `windowWillClose` 或控制權結束通知才發出關閉 callback。
- 修正後 TXT、合成 WAV 各通過 20 次真實面板循環，涵蓋 Space／Esc／關閉按鈕／另一視窗取得 main／立即清理；使用者後續人工複測確認本次 Quick Look 問題已修復。此結果不代表其餘完整人工驗收項目均已完成。
- `pnpm test`：34 files／201 tests 通過；`pnpm build` 通過（既有 chunk warnings）。`cargo test`：229 通過、5 個需協定 fixture 的測試略過；`cargo clippy --all-targets -- -D warnings`、格式及 diff 檢查通過。
- `pnpm tauri build --debug --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'` 通過，更新 `src-tauri/target/debug/bundle/macos/anySCP.app`，供人工複測。
- 暫存策略未新增跨次快取：`app_cache_dir()/quicklook-v1/<run UUID>/<preview UUID>/檔名`，面板解除引用、writer 結束後刪除該次 preview 目錄，失敗時重試／下次啟動清理；run 與 owner lock 保留至程式結束，root 與 sweep lock 可持續保留。重開同檔重新下載符合原 MVP 設計。
- 此項獨立修正不代表上方連結的 Code Review 中 R1～R6 已處理。
