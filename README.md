# 網路健檢（公務環境安全修正版）

- 預設只以 ICMP 探測預設閘道，不做外部探測、DNS 查詢或 traceroute。
- 經資訊人員同意，可在介面勾選外部測試：HiNet、Cloudflare 與公共網域 DNS 查詢。探測流量會讓外部服務看到來源 IP；未回應不能單獨證明線路故障。
- 經資訊人員同意，可另外勾選「分析附近的 Wi-Fi 環境」（預設不勾，與外部測試各自獨立）：讀取 Windows 已有的附近無線網路掃描結果（`netsh wlan show networks mode=bssid`），統計同頻道與頻道重疊的訊號源數量，判斷頻道是否擁擠，並在擁擠時提醒資訊人員評估換頻道。不會對外送出流量；附近網路的名稱與 MAC 位址只在解析當下存在記憶體，不存入狀態、畫面、CSV 或報告，報告只輸出頻道與數量統計。僅在使用 Wi-Fi 時有效；Windows 11 24H2 以上需開啟「位置服務」才讀得到。
- 手機 HTTP 服務、防火牆修改與 UAC 提權功能已從正式程式移除，原始碼不在這個分支的建置樹裡。舊版原始碼僅封存在本機分支 `archive/phone-test`（內含復活步驟與尚未處理的安全問題，見其中的 `ARCHIVE-NOTES.md`），未合併、未編譯、未註冊到應用程式。
- HTML 報告省略 SSID、網卡名稱、IP、節點主機名稱、原始錯誤訊息及完整檔案路徑；CSV 目標欄位固定為 `redacted`。時間與效能統計仍保留，分享前請依機關規範確認。
- 舊版已產生的報告與防火牆規則不會自動刪除，請交由資訊人員檢查處理。
- 診斷建議不再要求使用者重啟網路設備或改用公共 DNS。
- 報告與 CSV 仍存於「文件 / 網路健檢報告」，請依機關保存政策管理。

## 建置

需要 Windows、Rust MSVC、Visual Studio C++ Build Tools 與 Node.js。

```powershell
npm ci
npm run build
```

執行檔：`src-tauri/target/release/internet-test.exe`。
安裝檔：`src-tauri/target/release/bundle/nsis/`。

完整 `cargo test` 含會發送封包與操作暫存目錄的測試（`icmp::` 底下的測試會發 ICMP：本機迴圈、文件用保留位址 `192.0.2.1`、以及 TTL=1 發往 `1.1.1.1` 的封包，後者會在第一跳閘道過期），不應未經確認直接在公務環境執行。需要不發任何封包的版本時：

```powershell
cd src-tauri
cargo test -- --skip icmp::
```

## 本次驗證

- `node --check src/main.js` 通過。
- `node tests/security-ui.cjs` 通過：檢查 DOM 綁定、預設僅測閘道、外部測試勾選、停止與錯誤恢復；使用模擬 Tauri，不發送網路流量。
- 修改的 Rust 檔案通過 `rustfmt --check` 語法與格式檢查；這不等同編譯或型別檢查。
- 已加入報告去識別回歸測試。2026-10-04 在開發機上以 `cargo test -- --skip icmp::` 執行不發封包的測試：49 個通過（含報告去識別回歸測試）；`cargo clippy --all-targets` 可編譯，只剩既有警告。另在清理手機功能之前，曾於同一台機器執行過完整 `cargo test`（含 `icmp::`，會發封包）：56 個通過。
- 2026-10-06 加入 Wi-Fi 環境分析後，以 `cargo test -- --skip icmp::` 重跑不發封包的測試：79 個通過，無編譯警告。新增的測試涵蓋 `netsh` 輸出解析（中英文、有無頻段欄位、隱藏網路、同一台設備的多個網路名稱、位置權限被擋時的輸出）、擁擠度判斷與頻道建議、報告只在啟用時才出現該段落，以及結果資料結構不含任何網路名稱或位址。
- 2026-10-06 在 Windows 11 25H2（build 26200）上對照過真實輸出的欄位結構（只比對欄位名稱與值的類型，未檢視任何網路名稱或位址）：`show interfaces` 的基地台欄位在這個版本叫 `AP BSSID`，不是舊版的 `BSSID`，已修正並加回歸測試；`show networks mode=bssid` 的結構與解析器假設一致，該次的 16 個 BSSID 全部可解析。
- **尚未驗證：** 其他版本的 Windows 輸出格式可能有差異；上述對照是用相同規則在 PowerShell 重現，不是執行 App 本身，App 畫面仍需實機確認。擁擠度的門檻是第一版經驗法則，Windows 不回報頻寬，5 GHz 以 20 MHz 估算。
- 2026-10-06 已重新執行 `npm run build`（編譯成功），`target` 內的執行檔與安裝檔已是這次修正版，不再是含有手機 HTTP 服務與防火牆提權功能的舊建置。尚未實際啟動新版 EXE 做手動驗證，發送前請先確認。
