# 網路健檢（公務環境安全修正版）

- 預設只以 ICMP 探測預設閘道，不做外部探測、DNS 查詢或 traceroute。
- 經資訊人員同意，可在介面勾選外部測試：HiNet、Cloudflare 與公共網域 DNS 查詢。探測流量會讓外部服務看到來源 IP；未回應不能單獨證明線路故障。
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
- 2026-10-06 已重新執行 `npm run build`（編譯成功），`target` 內的執行檔與安裝檔已是這次修正版，不再是含有手機 HTTP 服務與防火牆提權功能的舊建置。尚未實際啟動新版 EXE 做手動驗證，發送前請先確認。
