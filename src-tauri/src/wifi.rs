//! Wi-Fi 訊號強度偵測。
//!
//! 家用環境裡「網路很慢」最常見的真正原因就是離分享器太遠。把訊號強度和
//! 延遲擺在一起看，才有辦法講出「不是網路壞掉，是你坐太遠了」。

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::Command;

use serde::Serialize;

/// 建立子行程時不要跳出黑色主控台視窗。
/// 這是 GUI 程式，閃一下 cmd 視窗會讓使用者以為中毒了。
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize)]
pub struct WifiStatus {
    /// 連線的無線網路名稱
    pub ssid: Option<String>,
    /// 訊號強度百分比（0-100）
    pub signal_pct: Option<u8>,
    /// 協商速率（Mbps），可看出是不是降速了
    pub rx_mbps: Option<u32>,
}

impl WifiStatus {
    /// 訊號是不是弱到會影響使用。
    ///
    /// Windows 回報的百分比大致對應 -50dBm(100%) 到 -100dBm(0%)。
    /// 40% 以下大約是 -80dBm，開始會有明顯掉包。
    pub fn is_weak(&self) -> bool {
        self.signal_pct.is_some_and(|p| p < 40)
    }

    /// 訊號強度的白話說法。
    pub fn plain_text(&self) -> Option<String> {
        let pct = self.signal_pct?;
        let word = match pct {
            0..=24 => "非常弱",
            25..=39 => "很弱",
            40..=59 => "普通",
            60..=79 => "良好",
            _ => "很強",
        };
        Some(match &self.ssid {
            Some(ssid) => format!("Wi-Fi「{ssid}」訊號{word}（{pct}%）"),
            None => format!("Wi-Fi 訊號{word}（{pct}%）"),
        })
    }
}

/// 以系統目錄下的 `netsh.exe` 執行指令，回傳標準輸出。任何一步失敗都回 None。
fn netsh(args: &[&str]) -> Option<String> {
    // Resolve the trusted system directory without searching PATH or the working directory.
    let mut buffer = vec![0u16; 32768];
    // SAFETY: buffer is writable and its capacity is passed in UTF-16 code units.
    let len = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if len == 0 || len >= buffer.len() {
        return None;
    }
    use std::os::windows::ffi::OsStringExt;
    let path =
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])).join("netsh.exe");
    let output = Command::new(path)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // netsh 在中文系統上輸出的是 ANSI 編碼（cp950），UTF-8 解碼會壞掉。
    // 這裡只需要 SSID 和數字，用 lossy 解碼就夠了 —— 壞掉的只會是
    // 「訊號」這類我們本來就不比對的中文標籤。
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 讀取目前的 Wi-Fi 狀態。
///
/// 走的是 `netsh wlan show interfaces`。這是外部指令，任何一步失敗
/// （沒有無線網卡、服務沒開、輸出格式不同）都只是回 None，不影響主要功能。
pub fn status() -> Option<WifiStatus> {
    parse(&netsh(&["wlan", "show", "interfaces"])?)
}

/// 解析 netsh 的輸出。
///
/// 刻意不比對中文/英文的欄位名稱 —— Windows 的顯示語言會變，
/// 但 `SSID` 這個鍵名不會翻譯，而訊號強度是唯一以 `%` 結尾的值。
fn parse(text: &str) -> Option<WifiStatus> {
    let mut ssid = None;
    let mut signal_pct = None;
    let mut rx_mbps = None;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }

        // 注意 BSSID 也含有 "SSID"，要用完全相等比對
        if key.eq_ignore_ascii_case("SSID") {
            ssid = Some(value.to_string());
            continue;
        }

        // 唯一以 % 結尾的欄位就是訊號強度
        if let Some(num) = value.strip_suffix('%') {
            if let Ok(pct) = num.trim().parse::<u8>() {
                signal_pct = Some(pct.min(100));
                continue;
            }
        }

        // 接收速率，欄位名稱含 Mbps（中英文版都保留這個單位）
        if key.contains("Mbps") && rx_mbps.is_none() {
            if let Ok(v) = value.parse::<f64>() {
                rx_mbps = Some(v.round() as u32);
            }
        }
    }

    // 完全沒抓到任何東西代表沒有連上 Wi-Fi（或沒有無線網卡）
    if ssid.is_none() && signal_pct.is_none() {
        return None;
    }

    Some(WifiStatus {
        ssid,
        signal_pct,
        rx_mbps,
    })
}

// ---------- 附近的無線訊號源 ----------

/// 無線頻段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Band {
    Ghz24,
    Ghz5,
    Ghz6,
}

impl Band {
    pub fn text(self) -> &'static str {
        match self {
            Band::Ghz24 => "2.4 GHz",
            Band::Ghz5 => "5 GHz",
            Band::Ghz6 => "6 GHz",
        }
    }
}

/// 附近的一個無線訊號源（實體基地台，不是每個網路名稱各算一個）。
///
/// 刻意不放網路名稱與 MAC 位址：那是別人的識別資訊，分析頻道擁擠度用不到，
/// 不存在欄位裡就不可能被帶進畫面、CSV 或報告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyAp {
    pub band: Band,
    pub channel: u16,
    pub signal_pct: u8,
    /// 是不是目前連線的那一台
    pub is_own: bool,
}

/// 讀不到附近無線環境的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanIssue {
    NotConnected,
    NoResult,
}

impl ScanIssue {
    pub fn message(self) -> &'static str {
        match self {
            ScanIssue::NotConnected => {
                "目前沒有連上 Wi-Fi，或系統不允許讀取無線資訊，無法分析附近的無線環境。"
            }
            ScanIssue::NoResult => {
                "沒有讀到附近的無線網路。Windows 11 24H2 以上需要開啟「位置服務」才能讀取，請交由資訊人員確認。"
            }
        }
    }
}

/// 讀取附近的無線訊號源。
///
/// 只讀取 Windows 已經有的掃描結果（`netsh wlan show networks mode=bssid`），
/// 程式本身不要求網卡重新掃描。需要目前連線的 BSSID 才分得出哪一個是自己的基地台。
pub fn scan_nearby() -> Result<Vec<NearbyAp>, ScanIssue> {
    let own = netsh(&["wlan", "show", "interfaces"])
        .and_then(|text| parse_own_bssid(&text))
        .ok_or(ScanIssue::NotConnected)?;
    let text = netsh(&["wlan", "show", "networks", "mode=bssid"]).ok_or(ScanIssue::NoResult)?;
    let aps = parse_scan(&text, &own);
    if aps.is_empty() {
        Err(ScanIssue::NoResult)
    } else {
        Ok(aps)
    }
}

/// 鍵是不是 `名稱` 或 `名稱 編號`（例如 `SSID`、`BSSID 2`）。這些英文鍵名 Windows 不會翻譯。
fn numbered_key(key: &str, name: &str) -> bool {
    key.len() >= name.len()
        && key.is_char_boundary(name.len())
        && key[..name.len()].eq_ignore_ascii_case(name)
        && key[name.len()..].trim().bytes().all(|b| b.is_ascii_digit())
}

/// 鍵名是不是 BSSID。舊版寫 `BSSID`、掃描清單寫 `BSSID 1`，
/// Windows 11 25H2 的 `show interfaces` 則寫成 `AP BSSID`，所以只看有沒有包含這個字。
/// 同樣是 MAC 格式的 `Physical address`（本機網卡）不含這個字，不會被誤認。
fn is_bssid_key(key: &str) -> bool {
    key.to_ascii_lowercase().contains("bssid")
}

/// 統一成小寫、冒號分隔；格式不對就回 None。
fn normalize_mac(raw: &str) -> Option<String> {
    let mac = raw.trim().to_ascii_lowercase().replace('-', ":");
    let ok = mac.split(':').count() == 6
        && mac
            .split(':')
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()));
    ok.then_some(mac)
}

/// 從 `show interfaces` 的輸出取出目前連線的 BSSID。
fn parse_own_bssid(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| is_bssid_key(key))
        .and_then(|(_, value)| normalize_mac(value))
}

fn band_from_value(value: &str) -> Option<Band> {
    let lower = value.to_ascii_lowercase();
    match lower.strip_suffix("ghz")?.trim() {
        "2.4" => Some(Band::Ghz24),
        "5" => Some(Band::Ghz5),
        "6" => Some(Band::Ghz6),
        _ => None,
    }
}

/// 舊版 Windows 的輸出沒有頻段欄位，只能從頻道號碼推測。
fn infer_band(channel: u16) -> Option<Band> {
    match channel {
        1..=14 => Some(Band::Ghz24),
        32..=177 => Some(Band::Ghz5),
        _ => None,
    }
}

fn channel_is_valid(band: Band, channel: u16) -> bool {
    match band {
        Band::Ghz24 => (1..=14).contains(&channel),
        Band::Ghz5 => (32..=177).contains(&channel),
        Band::Ghz6 => (1..=233).contains(&channel),
    }
}

/// 解析到一半的一個 BSSID 區塊。
struct RawBss {
    bssid: Option<String>,
    signal: Option<u8>,
    band: Option<Band>,
    channel: Option<u16>,
}

fn finish_bss(bss: Option<RawBss>, out: &mut Vec<(String, Band, u16, u8)>) {
    let Some(bss) = bss else { return };
    let (Some(bssid), Some(signal), Some(channel)) = (bss.bssid, bss.signal, bss.channel) else {
        return;
    };
    let Some(band) = bss.band.or_else(|| infer_band(channel)) else {
        return;
    };
    if channel_is_valid(band, channel) {
        out.push((bssid, band, channel, signal));
    }
}

/// 解析 `netsh wlan show networks mode=bssid` 的輸出。
///
/// 和 `parse` 一樣不比對中文/英文欄位名稱，改看結構：每個 `BSSID n` 底下，
/// 以 `%` 結尾的是訊號、含 `GHz` 的是頻段、第一個純整數是頻道
/// （頻道排在 BSS 負載的各項計數之前，速率欄位因為鍵名含 `Mbps` 而跳過）。
///
/// 同一台實體基地台常會用好幾個網路名稱（例如訪客網路），它們的 BSSID 通常只有
/// 最後一碼不同。這裡把「頻道相同、前 5 碼相同」的合併成一個，並且把自己那台
/// 基地台的其他網路名稱排除，否則會把自己的設備算成干擾。
pub fn parse_scan(text: &str, own_bssid: &str) -> Vec<NearbyAp> {
    let own = normalize_mac(own_bssid);
    let mut raws: Vec<(String, Band, u16, u8)> = Vec::new();
    let mut current: Option<RawBss> = None;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        // 先判斷網路名稱那一行：隱藏網路的名稱是空的，但仍然代表上一個區塊結束
        if numbered_key(key, "SSID") {
            finish_bss(current.take(), &mut raws);
            continue;
        }
        if value.is_empty() {
            continue;
        }
        if is_bssid_key(key) {
            finish_bss(current.take(), &mut raws);
            current = Some(RawBss {
                bssid: normalize_mac(value),
                signal: None,
                band: None,
                channel: None,
            });
            continue;
        }

        let Some(bss) = current.as_mut() else {
            continue;
        };
        if key.contains("Mbps") {
            continue;
        }
        if let Some(num) = value.strip_suffix('%') {
            if bss.signal.is_none() {
                if let Ok(pct) = num.trim().parse::<u8>() {
                    bss.signal = Some(pct.min(100));
                }
            }
            continue;
        }
        if let Some(band) = band_from_value(value) {
            bss.band.get_or_insert(band);
            continue;
        }
        if bss.channel.is_none() && value.bytes().all(|b| b.is_ascii_digit()) {
            bss.channel = value.parse::<u16>().ok();
        }
    }
    finish_bss(current.take(), &mut raws);

    // 前 5 碼代表同一台實體設備
    let device = |bssid: &str| {
        bssid
            .rsplit_once(':')
            .map_or("", |(head, _)| head)
            .to_string()
    };
    let own_key = own
        .as_deref()
        .and_then(|o| raws.iter().find(|r| r.0 == o))
        .map(|r| (r.1, r.2, device(&r.0)));

    let mut result = Vec::new();
    let mut others: HashMap<(Band, u16, String), u8> = HashMap::new();
    for (bssid, band, channel, signal) in &raws {
        if own.as_deref() == Some(bssid.as_str()) {
            result.push(NearbyAp {
                band: *band,
                channel: *channel,
                signal_pct: *signal,
                is_own: true,
            });
            continue;
        }
        let key = (*band, *channel, device(bssid));
        if own_key.as_ref() == Some(&key) {
            continue;
        }
        let best = others.entry(key).or_insert(*signal);
        *best = (*best).max(*signal);
    }
    result.extend(
        others
            .into_iter()
            .map(|((band, channel, _), signal)| NearbyAp {
                band,
                channel,
                signal_pct: signal,
                is_own: false,
            }),
    );

    // HashMap 的走訪順序不固定，排序後輸出才穩定
    result.sort_by_key(|a| (a.band, a.channel, a.signal_pct, a.is_own));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZH_TW: &str = r#"
系統上有 1 個介面:

    名稱                   : Wi-Fi
    描述                   : Intel(R) Wi-Fi 6 AX201 160MHz
    GUID                   : 1234abcd-0000-0000-0000-000000000000
    實體位址               : aa:bb:cc:dd:ee:ff
    狀態                   : 已連線
    SSID                   : MyHomeWiFi
    BSSID                  : 11:22:33:44:55:66
    網路類型               : 基礎結構
    無線電類型             : 802.11ax
    驗證                   : WPA2-Personal
    密碼                   : CCMP
    連線模式               : 自動連線設定檔
    通道                   : 36
    接收速率 (Mbps)        : 866.7
    傳輸速率 (Mbps)        : 866.7
    訊號                   : 78%
"#;

    const EN_US: &str = r#"
There is 1 interface on the system:

    Name                   : Wi-Fi
    State                  : connected
    SSID                   : Cafe Guest
    BSSID                  : 11:22:33:44:55:66
    Receive rate (Mbps)    : 144
    Signal                 : 31%
"#;

    #[test]
    fn parses_traditional_chinese_output() {
        let s = parse(ZH_TW).expect("應該解析成功");
        assert_eq!(s.ssid.as_deref(), Some("MyHomeWiFi"));
        assert_eq!(s.signal_pct, Some(78));
        assert_eq!(s.rx_mbps, Some(867));
        assert!(!s.is_weak());
    }

    #[test]
    fn parses_english_output() {
        let s = parse(EN_US).expect("應該解析成功");
        assert_eq!(s.ssid.as_deref(), Some("Cafe Guest"));
        assert_eq!(s.signal_pct, Some(31));
        assert!(s.is_weak(), "31% 應該算訊號弱");
    }

    #[test]
    fn bssid_is_not_mistaken_for_ssid() {
        let s = parse(ZH_TW).unwrap();
        assert_ne!(s.ssid.as_deref(), Some("11:22:33:44:55:66"));
    }

    #[test]
    fn ssid_containing_colon_keeps_everything_after_first_colon() {
        // SSID 允許含冒號，split_once 只切第一個是對的
        let s = parse("    SSID                   : Wi-Fi: 5G\n    Signal : 60%").unwrap();
        assert_eq!(s.ssid.as_deref(), Some("Wi-Fi: 5G"));
    }

    #[test]
    fn no_wireless_adapter_returns_none() {
        assert!(parse("There is 0 interface on the system.").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn plain_text_describes_signal_in_words() {
        let s = parse(ZH_TW).unwrap();
        let text = s.plain_text().unwrap();
        assert!(text.contains("MyHomeWiFi"), "{text}");
        assert!(text.contains("良好"), "{text}");
        // 畫面上不該出現 dBm 這種術語
        assert!(!text.contains("dBm"), "{text}");
    }

    // 新版 Windows 的輸出：有頻段與 BSS 負載。自己是 HomeNet（aa:bb:cc:00:00:01）。
    const SCAN_EN: &str = r#"
Interface name : Wi-Fi
There are 6 networks currently visible.

SSID 1 : HomeNet
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : aa:bb:cc:00:00:01
         Signal             : 88%
         Radio type         : 802.11ax
         Band               : 2.4 GHz
         Channel            : 6
         Bss Load:
            Connected Stations:         3
            Channel Utilization:        60 (23 %)
            Medium Available Capacity:  17000 (17000 us/s)
         Basic rates (Mbps) : 1 2 5.5 11
         Other rates (Mbps) : 6 9 12 18 24 36 48 54

SSID 2 : HomeNet-Guest
    Network type            : Infrastructure
    Authentication          : Open
    Encryption              : None
    BSSID 1                 : aa:bb:cc:00:00:02
         Signal             : 86%
         Radio type         : 802.11ax
         Band               : 2.4 GHz
         Channel            : 6

SSID 3 : Neighbor-A
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : 10:20:30:40:50:60
         Signal             : 75%
         Radio type         : 802.11n
         Band               : 2.4 GHz
         Channel            : 6
    BSSID 2                 : 10:20:30:40:50:61
         Signal             : 50%
         Radio type         : 802.11n
         Band               : 2.4 GHz
         Channel            : 6

SSID 4 :
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : 66:77:88:99:aa:01
         Signal             : 55%
         Radio type         : 802.11n
         Band               : 2.4 GHz
         Channel            : 4

SSID 5 : Neighbor-C
    Network type            : Infrastructure
    Authentication          : WPA2-Personal
    Encryption              : CCMP
    BSSID 1                 : 66:77:88:99:bb:01
         Signal             : 45%
         Radio type         : 802.11n
         Band               : 2.4 GHz
         Channel            : 1

SSID 6 : Neighbor-D-5G
    Network type            : Infrastructure
    Authentication          : WPA3-Personal
    Encryption              : CCMP
    BSSID 1                 : 12:34:56:78:9a:bc
         Signal             : 80%
         Radio type         : 802.11ac
         Band               : 5 GHz
         Channel            : 36
"#;

    // 中文介面、舊版輸出（沒有頻段欄位）。
    const SCAN_ZH: &str = r#"
介面名稱 : Wi-Fi
目前有 2 個可見的網路。

SSID 1 : 公司網路
    網路類型            : 基礎結構
    驗證                : WPA2-Enterprise
    加密                : CCMP
    BSSID 1             : AA-BB-CC-00-00-10
         訊號           : 70%
         無線電類型     : 802.11n
         通道           : 11
         基本速率 (Mbps) : 1 2 5.5 11
         其他速率 (Mbps) : 6 9 12 18 24 36 48 54

SSID 2 : Neighbor-E
    網路類型            : 基礎結構
    BSSID 1             : 11:22:33:44:55:77
         訊號           : 62%
         無線電類型     : 802.11n
         通道           : 11
"#;

    fn own_of(aps: &[NearbyAp]) -> Option<&NearbyAp> {
        aps.iter().find(|a| a.is_own)
    }

    #[test]
    fn scan_marks_own_ap_and_counts_each_physical_neighbor_once() {
        let aps = parse_scan(SCAN_EN, "aa:bb:cc:00:00:01");
        // 自己 + Neighbor-A（兩個 BSSID 合併） + 隱藏網路 + Neighbor-C + 5 GHz 那台。
        // 自己那台的訪客網路（…:02）不算成別人。
        assert_eq!(aps.len(), 5, "{aps:?}");

        let own = own_of(&aps).expect("應該認得出自己的基地台");
        assert_eq!(
            (own.band, own.channel, own.signal_pct),
            (Band::Ghz24, 6, 88)
        );
        assert_eq!(aps.iter().filter(|a| a.is_own).count(), 1);

        // Neighbor-A 的兩個 BSSID 合併後留下較強的那個
        assert!(aps.contains(&NearbyAp {
            band: Band::Ghz24,
            channel: 6,
            signal_pct: 75,
            is_own: false
        }));
        assert!(!aps.iter().any(|a| a.channel == 6 && a.signal_pct == 50));
        assert!(
            !aps.iter().any(|a| a.signal_pct == 86),
            "訪客網路不該被算進去"
        );

        // 隱藏網路（名稱是空的）仍然要被讀進來
        assert!(aps.iter().any(|a| a.channel == 4 && a.signal_pct == 55));
        assert!(aps.iter().any(|a| a.band == Band::Ghz5 && a.channel == 36));
    }

    #[test]
    fn scan_result_carries_no_network_identifiers() {
        let aps = parse_scan(SCAN_EN, "aa:bb:cc:00:00:01");
        let dump = format!("{aps:?}");
        for secret in ["Neighbor", "HomeNet", "10:20:30", "aa:bb:cc", "12:34:56"] {
            assert!(!dump.contains(secret), "{secret} 不該留在結果裡：{dump}");
        }
    }

    #[test]
    fn scan_parses_traditional_chinese_output_without_band_field() {
        let aps = parse_scan(SCAN_ZH, "aa-bb-cc-00-00-10");
        assert_eq!(aps.len(), 2, "{aps:?}");
        let own = own_of(&aps).unwrap();
        // 沒有頻段欄位時由頻道號碼推測；BSSID 用連字號也要認得
        assert_eq!(
            (own.band, own.channel, own.signal_pct),
            (Band::Ghz24, 11, 70)
        );
        assert!(aps.iter().any(|a| !a.is_own && a.signal_pct == 62));
    }

    #[test]
    fn bss_load_counters_are_not_mistaken_for_the_channel() {
        // Connected Stations 是 3、Channel Utilization 是 60，都不能蓋過頻道 6
        let aps = parse_scan(SCAN_EN, "aa:bb:cc:00:00:01");
        assert_eq!(own_of(&aps).unwrap().channel, 6);
    }

    #[test]
    fn scan_without_a_match_for_own_bssid_has_no_own_ap() {
        let aps = parse_scan(SCAN_EN, "de:ad:be:ef:00:01");
        assert!(own_of(&aps).is_none());
        assert!(!aps.is_empty());
    }

    #[test]
    fn location_permission_notice_yields_no_aps() {
        let text = "Network shell commands need location permission to access WLAN information.\nTurn on Location services on the Settings page.";
        assert!(parse_scan(text, "aa:bb:cc:00:00:01").is_empty());
        assert!(parse_scan("", "aa:bb:cc:00:00:01").is_empty());
    }

    #[test]
    fn impossible_channels_are_dropped() {
        let text = "SSID 1 : x\n    BSSID 1 : 10:20:30:40:50:60\n         Signal : 70%\n         Channel : 500\n";
        assert!(parse_scan(text, "aa:bb:cc:00:00:01").is_empty());
    }

    #[test]
    fn own_bssid_is_read_from_interface_output() {
        assert_eq!(parse_own_bssid(ZH_TW).as_deref(), Some("11:22:33:44:55:66"));
        assert_eq!(parse_own_bssid("SSID : x\nSignal : 50%"), None);
        assert_eq!(parse_own_bssid("BSSID : not-a-mac"), None);
    }

    // Windows 11 25H2（build 26200）實機的欄位結構，值全部換成假資料。
    // 基地台位址的鍵是 `AP BSSID`，不是舊版的 `BSSID`。
    const INTERFACES_25H2: &str = r#"
There is 1 interface on the system:

    Name                   : Wi-Fi
    Description            : Example Wi-Fi Adapter
    GUID                   : 00000000-0000-0000-0000-000000000000
    Physical address       : aa:bb:cc:dd:ee:ff
    Interface type         : Primary
    State                  : connected
    SSID                   : ExampleNetwork
    AP BSSID               : 11:22:33:44:55:66
    Band                   : 5 GHz
    Channel                : 36
    Connected Akm-cipher   : [Auth Algo = 802.11 Open, Akm = WPA2-PSK, Cipher = CCMP]
    Network type           : Infrastructure
    Radio type             : 802.11ax
    Authentication         : WPA2-Personal
    Cipher                 : CCMP
    Connection mode        : Auto Connect
    Receive rate (Mbps)    : 866.7
    Transmit rate (Mbps)   : 866.7
    Signal                 : 78%
    Rssi                   : -60
    Profile                : ExampleNetwork
"#;

    #[test]
    fn own_bssid_is_found_under_the_ap_bssid_key_of_windows_25h2() {
        // 不能把同樣是 MAC 格式的本機網卡位址（Physical address）當成基地台
        assert_eq!(
            parse_own_bssid(INTERFACES_25H2).as_deref(),
            Some("11:22:33:44:55:66")
        );
        // 原本讀 Wi-Fi 狀態的功能也要照常運作
        let s = parse(INTERFACES_25H2).expect("應該解析成功");
        assert_eq!(s.ssid.as_deref(), Some("ExampleNetwork"));
        assert_eq!(s.signal_pct, Some(78));
        assert_eq!(s.rx_mbps, Some(867));
    }

    #[test]
    fn bssid_keys_are_recognised_across_windows_versions() {
        for key in ["BSSID", "BSSID 2", "AP BSSID", "ap bssid", " BSSID "] {
            assert!(is_bssid_key(key), "{key}");
        }
        for key in ["Physical address", "實體位址", "SSID", "SSID 1", "Signal"] {
            assert!(!is_bssid_key(key), "{key}");
        }
    }

    #[test]
    fn numbered_keys_match_only_the_exact_name() {
        assert!(numbered_key("SSID", "SSID"));
        assert!(numbered_key("SSID 12", "SSID"));
        assert!(numbered_key("bssid 3", "BSSID"));
        assert!(!numbered_key("BSSID 1", "SSID"));
        assert!(!numbered_key("SSID name", "SSID"));
        assert!(!numbered_key("訊號", "SSID"));
    }
}
