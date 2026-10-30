# Quick Look Code Review 核對紀錄

- 日期：2026-10-30（Asia/Taipei）。
- 審查版本：`74b95e4..80b89d4`，共 14 個 commits；原始碼最後一次變更為 `81006c3`。
- 依使用者要求，以 Claude Code CLI `--model opus --effort high` 進行獨立 review，再沿用同一 session 做聚焦核對。
- 本次只審查與記錄，沒有修改功能程式碼。以下問題尚未修正。

## 核對後應處理的問題

### R1／P1：已有另一個 current request 時，close-before-open 仍會讓已取消的新請求復活

位置：`src-tauri/src/quicklook/mod.rs:219-230`。

可觸發順序：

1. 後端仍持有舊請求 `old/revision=1`。
2. 前端切到新檔案，依序發出 `old-close=2`、`new-open=3`，立即取消新檔案，再發出 `new-close=4`。
3. IPC 若先送達 `new-close=4`，因 current 仍是 old，`WindowState::close` 在身分不符分支直接返回，沒有記錄新請求已失效。
4. `new-open=3` 接著被接受，之後 `old-close=2` 被 revision 檢查丟棄。使用者已取消的新檔案仍可下載及彈窗。

這是主代理在核對時找到的反例。已抽取實際 `Identity`／`WindowState` 程式碼，以真實 Tokio mutex／CancellationToken 執行隔離 Rust 重現；僅縮小 `Request` 資料結構，未修改 repository。測試中的「已關閉請求不得重新接受」斷言失敗，exit code 101。它驗證的是後端狀態機，不代表已在真實 UI 重現。

可直接加入既有 `quicklook::tests` 的回歸情境：

```rust
let mut window = WindowState::default();
assert!(window.open(request("owner", "old", 1)));
window.close(&identity("owner", "new", 4));
assert!(!window.open(request("owner", "new", 3)));
```

修法要同時保留全域 revision watermark 與匹配 current 的清理能力。只把 watermark 提高到 4 還不夠：若 current 仍是 old/1，晚到的 old-close/2 仍須能清掉 old，不能因為小於 watermark 就無條件忽略。另需保留「舊 owner 的 close 不得取消新 owner」的既有保證。

### R2／P2：逾時後先等待 cleanup，排隊中的原生 show 仍可能執行

位置：`src-tauri/src/quicklook/mod.rs:479-502`，及主執行緒 show 檢查 `:538-541`。

`with_deadline` 返回 timeout 後，先執行 `scope.finish().await`，之後才取消 `request.cancel`。ChannelScope 取消的是 child token；實際 Tokio 原始碼明定取消 child 不會取消 parent。

如果 show 已派送但尚未執行，而 cleanup 正在等待 relay／channel close，主執行緒恢復時仍可能通過 `manager.current()` 並顯示面板。稍後 request 才失效並被 hide，形成已逾時卻短暫彈窗的行為。

修法方向：準備結果為錯誤時，在任何非同步 cleanup 前就讓 request 失效；成功顯示後等待使用者關閉的路徑則不能提前取消。這能移除等待 cleanup 的窗口；如需禁止在絕對期限已過之後才開始原生顯示，主執行緒也須核對同一 deadline，而不能只依賴稍後才被排程的取消操作。回歸測試應控制主執行緒派送與 cleanup barrier，推進 deadline 後斷言延遲 show 不得執行。

這項由主代理依實際控制流程與 Tokio 原始碼確認；本輪沒有執行真實 AppKit 的逾時彈窗重現。

### R3／P2：Quick Look 暫存初始化失敗會阻止整個 app 啟動

位置：`src-tauri/src/lib.rs:199-201`；`src-tauri/src/quicklook/staging.rs:66-73`。

cache 不可寫、既有 `quicklook-v1` 權限不符合 0700，或檔案系統不支援所需鎖時，`QuickLookManager::new` 會回傳錯誤，直接經 setup 的 `?` 傳出，最後導致 app build 失敗。這段在不支援 Quick Look 的 Linux／Windows 也會執行。

安全性檢查應保留，但選用預覽功能無法初始化時，應讓該功能不可用並保留 terminal／一般傳輸。建議維持已登錄的 manager 狀態並記錄 availability／初始化錯誤，或採惰性初始化；單純省略 `app.manage` 還必須一起處理所有 command、status listener、exit hook 的無條件 `state()` 存取。

來源：Claude 第一輪 F1；主代理以 setup 錯誤傳遞路徑確認，未以真實 app 注入 cache 失敗。

### R4／P2：啟動主執行緒同步等待 sweep flock 與清理

位置：`src-tauri/src/quicklook/staging.rs:73-103`，由 `lib.rs:199` 的同步 setup 呼叫。

全域 sweep lock 使用 blocking `lock_exclusive()`，且遞迴刪除 stale run 也在鎖內、主執行緒上執行。另一個程序持有 sweep lock，或其清理遭慢速 I/O 阻塞時，這個實例的啟動也會一起等待，沒有期限。

修法可採背景／惰性初始化並回報 preparing／unavailable，或重新切分持鎖區間。**不能只在 try-lock 失敗時略過鎖、照樣建立 run**：另一個正在 sweep 的程序可能在新 run 建好、owner lock 尚未建立時刪掉它。run 建立與 owner lock 的保護仍須保持原子性。

來源：Claude 第一輪 F2；blocking API 與 setup 呼叫鏈已核對，未進行真實多程序啟動的鎖阻塞重現。

### R5／P3：session 失效與預覽註冊沒有共用的有效期檢查

位置：`src-tauri/src/quicklook/mod.rs:379-404`，及 `open` 中先 bind、稍後註冊 request 的流程。

`close_session`／`close_ssh_session` 只取消當下已登錄的 request。若來源已 bind 到舊 session handle，失效通知在 request 註冊前發生，通知就找不到該請求；稍後 open 仍能登錄並使用保留的 handle。SFTP／SCP 的檔案 session 被移除後，共用 SSH 仍可能存活，因此舊預覽仍可繼續。

S3 bucket 名稱檢查也只在 bind 當下執行；已取得舊 bucket clone 後、註冊前的切換，不會被名稱檢查再次擋下。第一輪報告對「bucket switch 不受影響」的說法不足以排除此時序。

修法應讓 session／bucket 的 generation 或有效期與 open 註冊同步檢查。`close_session` 目前也用於 bucket switch，不能一律把 session id 永久列為 closed，否則同一 S3 session 切換後會永久失去預覽能力。只調換 remove／close 的順序也不能徹底消除 bind 與登錄之間的窗口。

來源：Claude 第一輪 F3，主代理核對並補充 bucket 情境與修法限制。正常前端的 invalidate／unmount 通知會降低觸發機率，但後端生命週期保證本身仍有缺口。

### R6／P3：立即原生關閉可能被回報為 cancelled，漏掉焦點還原

位置：`src-tauri/src/quicklook/mod.rs:479-501`；前端 `src/lib/quick-look.ts:144-147`。

主執行緒已顯示面板，卻在 tokio task 恢復前處理到原生關閉時，callback 會設定 `native_closed=true` 並取消 request。`current()` 因取消而變 false，成功結果可能落到 `Ok(()) => "cancelled"`；若 cancellation 與 show 回覆同時 ready，biased select 也可能先返回 `Err(cancelled)`。

前端只有 `closed` 終態會替 active request 執行焦點還原，故原生關閉可能被誤分類並漏掉還原。修法須涵蓋上述兩條分支，不能只改成功但非 current 的分支；仍要區分 owner/navigation 取消與真正原生關閉。

來源：Claude 第一輪 F4，主代理補充 biased select 的錯誤分支；本輪未執行真實 UI 快速關閉重現。

## 未採為當前缺陷的項目

- Claude F5：SCP 的 private file 建立與 staging helper 分開實作，確有重複與測試覆蓋缺口；但目前已是 `create_new`＋0600，沒有當前錯誤，不能把假設日後有人刪掉權限設定當成現有 bug。
- Claude F6：已銷毀 window 的 `u64::MAX` tombstone 超過 JS 安全整數；但目前沒有重建相同 label 的功能，且 tombstone 有意永久拒絕該已銷毀 window。列為未來 window 重建設計的限制，不採為現有可觸發問題；單純 clamp revision 也不會解決下一次 `next()` 超過上限。
- responder chain 被其他元件改寫、modal／舊 macOS 的 main-window 行為、nested run loop、webview reload 的 owner 重建，以及 fixture image 漂移等，仍屬待驗證疑點或維護建議，未與上述已具體指出時序的問題混列。

第二輪對「bucket 名稱檢查能排除所有 bucket switch 競態」的判斷仍未涵蓋已 bind、未註冊的情境，因此 R5 保留上述限制。這份紀錄採用經核對的技術結論，不直接照抄顧問建議。

## 測試、權限與交付狀態

- Claude 第一輪實際執行的是 git／搜尋／讀檔，沒有成功執行建置或測試。其自身權限層拒絕了 Cargo、Vitest 命令及部分工作目錄外的 dependency 原始碼存取。
- 主代理本輪額外執行 R1 的隔離 Rust 重現，確認斷言失敗，並核對實際 Tokio cancellation 原始碼。沒有把先前 201 項前端／229 項 Rust 通過結果當成本輪 review 發現已被修復的證據。
- 沒有修改功能、沒有重跑未變更的全套測試；上述問題尚待修正與加入正式回歸測試。
- Windows、macOS 11／Intel、完整 Explorer 到原生面板的人工驗收仍維持未驗證。

## Claude 呼叫紀錄

1. 獨立完整 review：要求閱讀全部 14 個 commits、計畫、核心傳輸與原生／前端生命週期，列具體情境、嚴重度、位置與回歸方式。回報 6 項 findings 及疑慮；核對後保留其中 4 項，另外 2 項不採為現有 bug。
2. 聚焦 follow-up：沿用同一 session，提供 R1 的實際反例與重現結果、R2 的取消順序、R6 的 biased-select 分支。Claude 明確承認原判斷有誤，同意三項成立；將 R1 評為 P1、R2 評為 P2，R6 維持 P3。也接受 F5／F6 不應列為當前 findings，撤回不安全的 try-lock 後直接建立 run，以及把 bucket switch 共用 session 永久封鎖的修法。第二輪沒有執行命令或測試。

本次 review 共呼叫 2 次；連同先前實作階段的 3 次，這項 Quick Look 工作共 5 次。詳細建置與先前驗證見 [原驗證紀錄](QuickLook-MVP-VALIDATION.md)。
