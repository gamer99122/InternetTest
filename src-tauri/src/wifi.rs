//! Wi-Fi 訊號強度偵測。
//!
//! 家用環境裡「網路很慢」最常見的真正原因就是離分享器太遠。把訊號強度和
//! 延遲擺在一起看，才有辦法講出「不是網路壞掉，是你坐太遠了」。

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

/// 讀取目前的 Wi-Fi 狀態。
///
/// 走的是 `netsh wlan show interfaces`。這是外部指令，任何一步失敗
/// （沒有無線網卡、服務沒開、輸出格式不同）都只是回 None，不影響主要功能。
pub fn status() -> Option<WifiStatus> {
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
        .args(["wlan", "show", "interfaces"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // netsh 在中文系統上輸出的是 ANSI 編碼（cp950），UTF-8 解碼會壞掉。
    // 這裡只需要 SSID 和數字，用 lossy 解碼就夠了 —— 壞掉的只會是
    // 「訊號」這類我們本來就不比對的中文標籤。
    let text = String::from_utf8_lossy(&output.stdout);
    parse(&text)
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
}
