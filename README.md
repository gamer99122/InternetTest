# 網路健檢

一個給**不懂電腦的人**使用的網路穩定度檢測工具。Rust + Tauri，單一執行檔，不需要系統管理員權限。

## 這個工具解決什麼問題

一般的 ping 工具只能回答「網路有沒有問題」，回答不了使用者真正想知道的
「**這是我家的問題，還是電信公司的問題？我該打電話給誰？**」

所以這個工具把網路拆成四段分別量測：

```
這台電腦 → 家裡的路由器 → 電信商 → 國內網路 → 國外網路
```

哪一段先壞掉，問題就出在那裡。畫面上會直接講出結論，例如：

> **網路會斷斷續續**
> 家裡的網路都正常，但中華電信這一段有問題。你家的網路設備正常，問題出在電信公司那一段。
> 建議：這不是你能修的，建議打電話給電信公司客服，並附上這份報告。

## 功能

| 功能 | 說明 |
|---|---|
| 分段監測 | 四段路徑各自量延遲、抖動、丟包，每秒一次 |
| 白話診斷 | 畫面上不出現任何技術術語，直接講「問題出在哪、該怎麼辦」 |
| 電信商識別 | 反查路徑節點的主機名稱，認得出中華電信、台灣大寬頻等業者 |
| 斷線紀錄 | 記錄每次斷線的開始時間與持續多久，累計穩定度 |
| 自動路徑追蹤 | 斷線當下自動跑 traceroute，事後看得出斷在哪一跳 |
| DNS 監測 | 很多「網路好慢」其實是查詢網址慢，連線本身沒問題 |
| Wi-Fi 訊號 | 把訊號強度和延遲關聯起來，能講出「你坐太遠了」 |
| 桌面通知 | 斷線與恢復時通知，可縮到系統匣長時間監測 |
| 一鍵報告 | 產生 HTML 報告存到「文件」並自動打開，可直接傳給客服 |
| CSV 紀錄 | 逐筆原始數據，需要深入分析時用 Excel 打開 |

## 開發環境需求

- **Rust**（stable-x86_64-pc-windows-msvc）
- **Visual Studio Build Tools** 的「使用 C++ 的桌面開發」工作負載
- **Node.js**（只用來跑 Tauri CLI，前端沒有打包步驟）

安裝 Build Tools：

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

## 開發與建置

```powershell
npm install          # 只需要跑一次

npm run dev          # 開發模式，改前端檔案會自動重新載入
npm run build        # 建置正式版

cd src-tauri
cargo test           # 跑單元測試
```

建置產物：

- `src-tauri/target/release/internet-test.exe` — 單一執行檔，複製到任何 Windows 電腦雙擊即可執行
- `src-tauri/target/release/bundle/nsis/` — 安裝檔，會建立開始功能表捷徑

## 設計上的取捨

**為什麼用 `IcmpSendEcho` 而不是 raw socket**
raw socket 在 Windows 上需要系統管理員權限。要求一個不懂電腦的人「右鍵、以系統管理員身分執行」是行不通的，所以走 iphlpapi 的 `IcmpSendEcho`，一般權限就能發 ICMP。`icmp.rs` 裡有一個測試專門守著這件事。

**為什麼延遲用 `Instant` 量而不是用 API 回報的 `RoundTripTime`**
後者只有整數毫秒的解析度，區域網路內幾乎永遠回報 0，那樣就完全算不出抖動了。

**為什麼電信商那一段用 traceroute 找而不是寫死一個 IP**
每家電信商、每個地區的機房位址都不一樣。用 traceroute 找「第一個非私有位址的節點」才是通用的做法。

**為什麼所有文字都在 Rust 端組好**
如果前端也做門檻判斷，同一套規則就會散在兩個地方，改一邊忘了改另一邊。前端只負責畫。

**為什麼不跳存檔對話框**
要一個不懂電腦的人在檔案總管裡選資料夾，比直接告訴他「已經存到文件裡了」困難得多。報告固定存到「文件 / 網路健檢報告」，存完自動打開。

## 專案結構

```
src/                    前端（純 HTML/CSS/JS，無打包工具）
  index.html
  styles.css
  main.js

src-tauri/src/
  main.rs               Tauri 進入點、指令、系統匣
  monitor.rs            監測引擎，每秒探測、推送狀態
  verdict.rs            把數據翻譯成白話結論 ← 這個工具的核心
  targets.rs            四段拓撲偵測、電信商識別
  icmp.rs               Windows ICMP FFI（免管理員權限）
  traceroute.rs         逐跳路徑追蹤
  stats.rs              統計與分級
  dns.rs                DNS 解析計時與反向查詢
  wifi.rs               Wi-Fi 訊號強度
  storage.rs            CSV 紀錄
  report.rs             HTML 報告產生
```
