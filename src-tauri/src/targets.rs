//! 把「網路」拆成四段可以分別量測的路徑。
//!
//! 只 ping 一個 8.8.8.8 只能回答「有沒有問題」，回答不了「問題出在哪」。
//! 拆成四段之後就能講出「你家網路正常，是中華電信那邊在塞車」這種話 ——
//! 這才是使用者真正想知道的事。

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use serde::Serialize;

use crate::traceroute::{self, is_private_v4};

/// 網路路徑上的四個段落，由近到遠。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// 這台電腦到家裡的路由器
    Router,
    /// 家裡到電信商機房的第一台設備
    Isp,
    /// 到國內網路
    Domestic,
    /// 到國外網路
    International,
}

impl SegmentKind {
    /// 預設顯示名稱（找不到更具體的名字時使用）。
    pub fn default_label(self) -> &'static str {
        match self {
            SegmentKind::Router => "家裡的網路",
            SegmentKind::Isp => "電信商",
            SegmentKind::Domestic => "國內網路",
            SegmentKind::International => "國外網路",
        }
    }

    /// 這一段在做什麼，寫給使用者看的一句話。
    pub fn description(self) -> &'static str {
        match self {
            SegmentKind::Router => "你的電腦連到家裡的無線分享器或數據機",
            SegmentKind::Isp => "從你家連到電信公司的機房",
            SegmentKind::Domestic => "連到台灣國內的網站",
            SegmentKind::International => "連到國外的網站，像是 YouTube、Google",
        }
    }

    /// 這一段壞掉時，該怪誰。這是整個工具最重要的一句話。
    pub fn blame(self) -> &'static str {
        match self {
            SegmentKind::Router => "問題出在你家裡 —— 可能是 Wi-Fi 訊號不好，或是分享器該重開機了",
            SegmentKind::Isp => "你家的網路設備正常，問題出在電信公司那一段",
            SegmentKind::Domestic => "連到國內網站的線路有問題",
            SegmentKind::International => "國內連線正常，但連到國外的線路在塞車",
        }
    }

    /// 這一段壞掉時，可以建議使用者做什麼。
    pub fn suggestion(self) -> &'static str {
        match self {
            SegmentKind::Router => "可以先試著把無線分享器的電源拔掉，等 30 秒再插回去",
            SegmentKind::Isp => "這不是你能修的，建議打電話給電信公司客服，並附上這份報告",
            SegmentKind::Domestic => "如果只有特定網站連不上，通常是那個網站自己的問題",
            SegmentKind::International => "看國外影片可能會卡，但國內網站應該還是正常的",
        }
    }
}

/// 一個實際要探測的目標。
#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub kind: SegmentKind,
    pub addr: Ipv4Addr,
    /// 顯示給使用者的名稱，例如「中華電信」
    pub label: String,
    /// 技術細節，只出現在報告與 CSV 裡
    pub detail: String,
}

/// 這台機器目前的網路拓撲。
#[derive(Debug, Clone, Serialize)]
pub struct Topology {
    pub segments: Vec<Segment>,
    /// 目前使用的網路卡名稱
    pub interface_name: String,
    /// 是不是走 Wi-Fi（會影響診斷建議）
    pub is_wifi: bool,
    pub local_ip: Option<Ipv4Addr>,
    pub gateway: Option<Ipv4Addr>,
    /// 認出來的電信商名稱
    pub isp_name: Option<String>,
    /// 偵測過程中遇到的狀況，顯示在報告裡
    pub notes: Vec<String>,
}

/// 國內段的固定目標：HiNet 的公用 DNS，在台灣幾乎不會不通。
const DOMESTIC_TARGET: Ipv4Addr = Ipv4Addr::new(168, 95, 1, 1);
/// 國外段的固定目標：Cloudflare 的公用 DNS。
const INTERNATIONAL_TARGET: Ipv4Addr = Ipv4Addr::new(1, 1, 1, 1);
/// 找不到電信商節點時的退路。
const ISP_FALLBACK: Ipv4Addr = Ipv4Addr::new(168, 95, 192, 1);

/// 從主機名稱認出是哪一家電信商。
///
/// 比對的是網域字尾，這樣 `h254.s98.ts.hinet.net` 也能認出是中華電信。
pub fn isp_from_hostname(hostname: &str) -> Option<&'static str> {
    // 由長到短比對，避免 `net.tw` 這類短字尾先中
    const TABLE: &[(&str, &str)] = &[
        ("hinet.net", "中華電信"),
        ("cht.com.tw", "中華電信"),
        ("so-net.net.tw", "So-net"),
        ("seed.net.tw", "SEEDNet"),
        ("kbtelecom.net", "凱擘大寬頻"),
        ("kbro.com.tw", "凱擘大寬頻"),
        ("twmbroadband.net", "台灣大寬頻"),
        ("tfn.net.tw", "台灣大哥大"),
        ("taiwanmobile.com", "台灣大哥大"),
        ("fetnet.net", "遠傳電信"),
        ("aptg.net.tw", "亞太電信"),
        ("apol.com.tw", "亞太電信"),
        ("dstw.net", "數位聯合 Seednet"),
        ("gcn.net.tw", "大新竹有線電視"),
        ("twaren.net", "學術網路 TWAREN"),
        ("edu.tw", "學術網路 TANet"),
    ];

    let host = hostname.trim_end_matches('.').to_ascii_lowercase();
    TABLE
        .iter()
        .find(|(suffix, _)| host == *suffix || host.ends_with(&format!(".{suffix}")))
        .map(|(_, name)| *name)
}

/// 偵測目前的網路環境，決定四段各要打哪裡。
///
/// 這個函式會在開始監測前跑一次。任何一步失敗都不會中斷整體流程，
/// 只是那一段會被跳過或退回預設值，並在 `notes` 留下說明。
pub async fn detect() -> Topology {
    let mut notes = Vec::new();
    let mut segments = Vec::new();

    // --- 網路卡與本機位址 ---
    let (interface_name, is_wifi, local_ip) = match netdev::get_default_interface() {
        Ok(iface) => {
            let name = iface
                .friendly_name
                .clone()
                .or_else(|| iface.description.clone())
                .unwrap_or_else(|| iface.name.clone());
            let wifi = matches!(
                iface.if_type,
                netdev::interface::types::InterfaceType::Wireless80211
            );
            let ip = iface.ipv4.first().map(|n| n.addr());
            (name, wifi, ip)
        }
        Err(e) => {
            notes.push(format!("找不到使用中的網路卡：{e}"));
            (String::from("未知"), false, None)
        }
    };

    // --- 第一段：家裡的路由器 ---
    let gateway = match netdev::get_default_gateway() {
        Ok(gw) => gw.ipv4.first().copied(),
        Err(e) => {
            notes.push(format!("找不到預設閘道：{e}"));
            None
        }
    };

    if let Some(gw) = gateway {
        segments.push(Segment {
            kind: SegmentKind::Router,
            addr: gw,
            label: SegmentKind::Router.default_label().to_string(),
            detail: format!("預設閘道 {gw}（{interface_name}）"),
        });
    } else {
        notes.push("偵測不到家裡的路由器，可能是網路線沒插好或沒連上 Wi-Fi".into());
    }

    // --- 第二段：電信商的第一台設備 ---
    // 用 traceroute 找出第一個公網位址。這比猜一個固定 IP 準確得多，
    // 因為每家電信商、每個地區的機房位址都不一樣。
    let mut isp_name = None;
    let isp_addr = match traceroute::trace(
        INTERNATIONAL_TARGET,
        6,
        Duration::from_millis(800),
        1,
    )
    .await
    {
        Ok(hops) => match traceroute::first_isp_hop(&hops) {
            Some(hop) => hop.addr,
            None => {
                notes.push("追蹤不到電信商的節點，改用預設位址監測這一段".into());
                None
            }
        },
        Err(e) => {
            notes.push(format!("路徑追蹤失敗：{e}"));
            None
        }
    };

    let isp_addr = match isp_addr {
        Some(a) => a,
        // 退路：系統設定的 DNS 若是公網位址就用它，否則用 HiNet
        None => system_public_dns().unwrap_or(ISP_FALLBACK),
    };

    // 反查主機名稱，認出電信商叫什麼名字
    let isp_label = match crate::dns::reverse(isp_addr).await {
        Some(host) => {
            let name = isp_from_hostname(&host);
            isp_name = name.map(|s| s.to_string());
            name.unwrap_or(SegmentKind::Isp.default_label()).to_string()
        }
        None => SegmentKind::Isp.default_label().to_string(),
    };

    segments.push(Segment {
        kind: SegmentKind::Isp,
        addr: isp_addr,
        label: isp_label,
        detail: format!("電信商第一跳 {isp_addr}"),
    });

    // --- 第三、四段：國內與國外 ---
    segments.push(Segment {
        kind: SegmentKind::Domestic,
        addr: DOMESTIC_TARGET,
        label: SegmentKind::Domestic.default_label().to_string(),
        detail: format!("HiNet 公用 DNS {DOMESTIC_TARGET}"),
    });
    segments.push(Segment {
        kind: SegmentKind::International,
        addr: INTERNATIONAL_TARGET,
        label: SegmentKind::International.default_label().to_string(),
        detail: format!("Cloudflare 公用 DNS {INTERNATIONAL_TARGET}"),
    });

    Topology {
        segments,
        interface_name,
        is_wifi,
        local_ip,
        gateway,
        isp_name,
        notes,
    }
}

/// 系統設定的 DNS 伺服器裡，第一個公網 IPv4 位址。
///
/// 家用路由器通常會把自己設成 DNS（192.168.x.1），那種對「電信商這一段」
/// 沒有參考價值，所以私有位址要濾掉。
fn system_public_dns() -> Option<Ipv4Addr> {
    crate::dns::system_dns_servers()
        .into_iter()
        .filter_map(|ip| match ip {
            IpAddr::V4(v4) => Some(v4),
            IpAddr::V6(_) => None,
        })
        .find(|v4| !is_private_v4(*v4))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_isp_from_subdomain() {
        assert_eq!(
            isp_from_hostname("h254.s98.ts.hinet.net"),
            Some("中華電信")
        );
        assert_eq!(isp_from_hostname("HINET.NET"), Some("中華電信"));
        assert_eq!(
            isp_from_hostname("core1.tp.twmbroadband.net."),
            Some("台灣大寬頻")
        );
    }

    #[test]
    fn unknown_hostnames_return_none() {
        assert_eq!(isp_from_hostname("router.example.com"), None);
        assert_eq!(isp_from_hostname(""), None);
    }

    #[test]
    fn suffix_match_does_not_match_partial_labels() {
        // 「evilhinet.net」不是中華電信，不能因為字串包含就誤判
        assert_eq!(isp_from_hostname("evilhinet.net"), None);
        // 但正確的子網域要認得出來
        assert_eq!(isp_from_hostname("a.evil.hinet.net"), Some("中華電信"));
    }

    #[test]
    fn every_segment_kind_has_plain_language_strings() {
        for k in [
            SegmentKind::Router,
            SegmentKind::Isp,
            SegmentKind::Domestic,
            SegmentKind::International,
        ] {
            assert!(!k.default_label().is_empty());
            assert!(!k.description().is_empty());
            assert!(!k.blame().is_empty());
            assert!(!k.suggestion().is_empty());
        }
    }

    #[test]
    fn segment_kinds_are_ordered_near_to_far() {
        // 順序有意義：歸因時要從最近的一段開始怪罪
        assert!(SegmentKind::Router < SegmentKind::Isp);
        assert!(SegmentKind::Isp < SegmentKind::Domestic);
        assert!(SegmentKind::Domestic < SegmentKind::International);
    }
}
