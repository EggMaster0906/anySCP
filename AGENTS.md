Claude Code CLI — 外部 Agent 諮詢

當遇到難以解決的問題、需要第二意見或技術方案評估時，可以透過 Claude Code CLI 進行非互動式諮詢。

1. 發起新對話

claude -p "你的問題" --model opus --effort high

* -p：非互動式執行，完成後直接輸出結果。
* --model：指定模型，直接使用上述範例提供的直接使用上述範例提供的 opus 模型
* --effort：指定思考強度，直接使用上述範例提供的 high 參數

2. 延續上一段對話

claude -p "後續問題" --continue --model opus --effort high

--continue 會延續目前工作目錄中最近一次 Claude Code 對話，保留先前的對話上下文。

3. 延續指定對話

claude -p "後續問題" --resume <SESSION_ID> --model opus --effort high

--resume 可指定 Session ID，避免同時存在多個 Claude 對話時接續錯誤的上下文。

使用原則

* 優先使用非互動式模式，避免啟動互動式 CLI。
* 同一問題的後續討論應延續原對話，避免重複提供上下文。
* Claude 的回覆僅作為技術參考，應自行驗證後再採用。
