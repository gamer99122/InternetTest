//! 附近無線環境的擁擠度分析。
//!
//! 只處理 `wifi::NearbyAp` 的頻段、頻道與訊號強度，產出的是數量與等級的聚合結果，
//! 不含任何網路名稱或位址。所有給使用者看的文字都在這裡組好，前端只負責畫。
//!
//! 這是第一版的經驗法則，不是量測：
//! - Windows 不回報頻寬，5 GHz / 6 GHz 一律以 20 MHz 估算（只有同頻道才算重疊）。
//! - 掃描結果是系統某個時間點的快照，不是即時資料。
//! - 「重疊」只代表有搶頻道的條件，不代表一定造成不穩。

use serde::Serialize;

use crate::wifi::{Band, NearbyAp};

/// 訊號達到這個強度才算「明顯」。Windows 的 40% 約等於 -80 dBm，
/// 和 `WifiStatus::is_weak` 用同一條線。
const SIGNIFICANT_PCT: u8 = 40;
/// 60% 約 -70 dBm，這個強度的鄰居已經會明顯和你搶頻道。
const STRONG_PCT: u8 = 60;

/// 找不到自己那台基地台時的說明（可能剛好換了基地台）。
pub const OWN_AP_MISSING: &str =
    "找不到目前連線的無線基地台，這一輪無法判斷頻道（可能剛好切換了基地台）。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Clear,
    Moderate,
    Crowded,
}

/// 一次分析的結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Congestion {
    pub level: Level,
    pub band: Band,
    pub own_channel: u16,
    /// 同頻道、訊號明顯的其他訊號源
    pub same_channel: usize,
    /// 頻道有重疊、訊號明顯的其他訊號源（2.4 GHz 相鄰頻道也會重疊）
    pub overlapping: usize,
    /// 其中訊號很強的
    pub strong: usize,
    /// 同頻段看到的其他訊號源總數，不論訊號強弱
    pub visible_others: usize,
    /// 明顯比較空的頻道
    pub suggested_channel: Option<u16>,
}

/// 2.4 GHz 的頻道間隔只有 5 MHz、寬度約 20 MHz，所以差 4 以內都會互相干擾。
fn overlaps(band: Band, a: u16, b: u16) -> bool {
    match band {
        Band::Ghz24 => a.abs_diff(b) <= 4,
        Band::Ghz5 | Band::Ghz6 => a == b,
    }
}

/// 建議頻道只從不需要雷達偵測（DFS）的頻道裡挑，換過去才不會被系統踢回來。
/// 6 GHz 的規範各地不同，這一版不給建議。
fn candidates(band: Band) -> &'static [u16] {
    match band {
        Band::Ghz24 => &[1, 6, 11],
        Band::Ghz5 => &[36, 40, 44, 48, 149, 153, 157, 161],
        Band::Ghz6 => &[],
    }
}

/// 某個頻道上受到的干擾量：所有重疊的明顯訊號源的強度總和。
fn load(band: Band, channel: u16, others: &[&NearbyAp]) -> u32 {
    others
        .iter()
        .filter(|a| a.signal_pct >= SIGNIFICANT_PCT && overlaps(band, channel, a.channel))
        .map(|a| u32::from(a.signal_pct))
        .sum()
}

/// 分析附近的無線環境。找不到自己的基地台時回 None（沒有基準就無從比較）。
pub fn analyze(aps: &[NearbyAp]) -> Option<Congestion> {
    let own = aps.iter().find(|a| a.is_own)?;
    // 只有同頻段的才會互相干擾
    let others: Vec<&NearbyAp> = aps
        .iter()
        .filter(|a| !a.is_own && a.band == own.band)
        .collect();

    let loud = |a: &&&NearbyAp| a.signal_pct >= SIGNIFICANT_PCT;
    let same_channel = others
        .iter()
        .filter(loud)
        .filter(|a| a.channel == own.channel)
        .count();
    let overlapping: Vec<&&NearbyAp> = others
        .iter()
        .filter(loud)
        .filter(|a| overlaps(own.band, own.channel, a.channel))
        .collect();
    let strong = overlapping
        .iter()
        .filter(|a| a.signal_pct >= STRONG_PCT)
        .count();

    let level = if strong >= 2 || overlapping.len() >= 5 {
        Level::Crowded
    } else if strong >= 1 || overlapping.len() >= 2 {
        Level::Moderate
    } else {
        Level::Clear
    };

    // 只有真的擁擠才建議換，而且要明顯更空（干擾量至少減半）才值得
    let suggested_channel = if level == Level::Clear {
        None
    } else {
        let own_load = load(own.band, own.channel, &others);
        candidates(own.band)
            .iter()
            .copied()
            .filter(|&c| c != own.channel)
            .min_by_key(|&c| (load(own.band, c, &others), c))
            .filter(|&c| load(own.band, c, &others) * 2 <= own_load)
    };

    Some(Congestion {
        level,
        band: own.band,
        own_channel: own.channel,
        same_channel,
        overlapping: overlapping.len(),
        strong,
        visible_others: others.len(),
        suggested_channel,
    })
}

/// 推給前端的呈現。
#[derive(Debug, Clone, Serialize)]
pub struct WifiEnvView {
    /// 「不擁擠」「有些擁擠」「擁擠」，或「讀取中」「無法分析」
    pub status_text: String,
    /// CSS class：good / fair / poor；沒有判斷時是空字串
    pub color: String,
    pub detail: String,
    /// 和目前連線穩定度的對照說明
    pub note: Option<String>,
    pub suggestion: Option<String>,
}

impl WifiEnvView {
    pub fn waiting() -> Self {
        WifiEnvView {
            status_text: "讀取中".into(),
            color: String::new(),
            detail: "正在讀取附近的無線網路，請稍候。".into(),
            note: None,
            suggestion: None,
        }
    }

    pub fn unavailable(reason: &str) -> Self {
        WifiEnvView {
            status_text: "無法分析".into(),
            color: String::new(),
            detail: reason.into(),
            note: None,
            suggestion: None,
        }
    }
}

impl Congestion {
    /// `link_unstable`：到閘道那一段目前是不是不穩定。
    /// 擁擠和不穩同時出現才值得提醒使用者留意關聯；兩者對不上時也要說，避免誤導。
    pub fn view(&self, link_unstable: bool) -> WifiEnvView {
        let (status_text, color) = match self.level {
            Level::Clear => ("不擁擠", "good"),
            Level::Moderate => ("有些擁擠", "fair"),
            Level::Crowded => ("擁擠", "poor"),
        };

        let band = self.band.text();
        let mut detail = format!("目前連在 {band} 的 {} 號頻道。", self.own_channel);
        if self.band == Band::Ghz24 {
            detail.push_str(&format!(
                "同頻道有 {} 個明顯的其他訊號源，頻道有重疊的共 {} 個（其中 {} 個訊號很強）。",
                self.same_channel, self.overlapping, self.strong
            ));
        } else {
            detail.push_str(&format!(
                "同頻道有 {} 個明顯的其他訊號源（其中 {} 個訊號很強）。",
                self.same_channel, self.strong
            ));
        }
        detail.push_str(&format!(
            "同頻段一共看到 {} 個其他訊號源。",
            self.visible_others
        ));

        let note = match (link_unstable, self.level) {
            (true, Level::Clear) => Some(
                "到閘道這一段目前不穩定，但頻道並不擁擠，較可能是其他原因，例如距離、牆面或設備本身。",
            ),
            (true, _) => Some(
                "到閘道這一段目前也不穩定，可能與無線環境擁擠有關；但這份資料只能顯示「同時發生」，無法證明因果。",
            ),
            (false, Level::Clear) => None,
            (false, _) => Some("目前連線仍然穩定，這個擁擠程度暫時沒有造成明顯影響。"),
        };

        let suggestion = match (self.level, self.suggested_channel) {
            (Level::Clear, _) => None,
            (_, Some(c)) if self.band == Band::Ghz24 => Some(format!(
                "可請機關資訊人員評估改用較空的 {c} 號頻道，或在設備支援時改用 5 GHz 頻段。"
            )),
            (_, Some(c)) => Some(format!(
                "可請機關資訊人員評估改用較空的頻道（例如 {c} 號）。"
            )),
            (_, None) if self.band == Band::Ghz24 => {
                Some("可請機關資訊人員評估在設備支援時改用 5 GHz 頻段，干擾通常較少。".to_string())
            }
            (_, None) => Some("可請機關資訊人員評估附近訊號源的分布。".to_string()),
        };

        WifiEnvView {
            status_text: status_text.into(),
            color: color.into(),
            detail,
            note: note.map(Into::into),
            suggestion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(band: Band, channel: u16, signal_pct: u8) -> NearbyAp {
        NearbyAp {
            band,
            channel,
            signal_pct,
            is_own: false,
        }
    }

    fn own(band: Band, channel: u16) -> NearbyAp {
        NearbyAp {
            is_own: true,
            ..ap(band, channel, 80)
        }
    }

    #[test]
    fn without_own_ap_there_is_nothing_to_compare_against() {
        assert!(analyze(&[ap(Band::Ghz24, 6, 70)]).is_none());
        assert!(analyze(&[]).is_none());
    }

    #[test]
    fn empty_air_is_clear_and_needs_no_suggestion() {
        let c = analyze(&[own(Band::Ghz24, 6), ap(Band::Ghz24, 11, 90)]).unwrap();
        assert_eq!(c.level, Level::Clear);
        assert_eq!(c.suggested_channel, None);
        assert_eq!(c.overlapping, 0);
        assert!(c.view(false).suggestion.is_none());
    }

    #[test]
    fn two_strong_neighbors_on_the_same_channel_is_crowded() {
        let c = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 6, 75),
            ap(Band::Ghz24, 6, 65),
        ])
        .unwrap();
        assert_eq!(c.level, Level::Crowded);
        assert_eq!((c.same_channel, c.overlapping, c.strong), (2, 2, 2));
    }

    #[test]
    fn adjacent_channels_overlap_on_2_4_ghz_but_not_on_5_ghz() {
        let neighbors = |band| {
            vec![
                own(band, if band == Band::Ghz24 { 6 } else { 36 }),
                ap(band, if band == Band::Ghz24 { 4 } else { 40 }, 70),
            ]
        };
        // 2.4 GHz：6 和 4 差 2，會互相干擾，但不是「同頻道」
        let c24 = analyze(&neighbors(Band::Ghz24)).unwrap();
        assert_eq!((c24.same_channel, c24.overlapping), (0, 1));
        assert_eq!(c24.level, Level::Moderate);
        // 5 GHz：36 和 40 是各自獨立的頻道
        let c5 = analyze(&neighbors(Band::Ghz5)).unwrap();
        assert_eq!((c5.same_channel, c5.overlapping), (0, 0));
        assert_eq!(c5.level, Level::Clear);
    }

    #[test]
    fn channels_five_apart_on_2_4_ghz_do_not_overlap() {
        let c = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 1, 90),
            ap(Band::Ghz24, 11, 90),
        ])
        .unwrap();
        assert_eq!(c.overlapping, 0);
        assert_eq!(c.visible_others, 2);
    }

    #[test]
    fn weak_neighbors_are_seen_but_do_not_count_as_interference() {
        let c = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 6, 20),
            ap(Band::Ghz24, 6, 39),
        ])
        .unwrap();
        assert_eq!(c.overlapping, 0);
        assert_eq!(c.visible_others, 2);
        assert_eq!(c.level, Level::Clear);
    }

    #[test]
    fn other_bands_are_ignored() {
        let c = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz5, 6, 90),
            ap(Band::Ghz6, 6, 90),
        ])
        .unwrap();
        assert_eq!(c.visible_others, 0);
        assert_eq!(c.level, Level::Clear);
    }

    #[test]
    fn suggests_the_emptiest_of_the_three_clean_2_4_ghz_channels() {
        // 自己在 6，被兩台強訊號壓著；1 上有一台、11 是空的
        let c = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 6, 80),
            ap(Band::Ghz24, 6, 70),
            ap(Band::Ghz24, 1, 60),
        ])
        .unwrap();
        assert_eq!(c.level, Level::Crowded);
        assert_eq!(c.suggested_channel, Some(11));
    }

    #[test]
    fn no_suggestion_when_every_alternative_is_just_as_busy() {
        let c = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 6, 70),
            ap(Band::Ghz24, 6, 65),
            ap(Band::Ghz24, 1, 70),
            ap(Band::Ghz24, 1, 65),
            ap(Band::Ghz24, 11, 70),
            ap(Band::Ghz24, 11, 65),
        ])
        .unwrap();
        assert_eq!(c.level, Level::Crowded);
        assert_eq!(c.suggested_channel, None);
        // 沒有更空的頻道，就只建議評估換頻段
        let text = c.view(false).suggestion.unwrap();
        assert!(text.contains("5 GHz"), "{text}");
    }

    #[test]
    fn five_ghz_suggestion_only_uses_channels_without_radar_detection() {
        let c = analyze(&[
            own(Band::Ghz5, 36),
            ap(Band::Ghz5, 36, 80),
            ap(Band::Ghz5, 36, 70),
        ])
        .unwrap();
        assert_eq!(c.level, Level::Crowded);
        let suggested = c.suggested_channel.expect("5 GHz 有很多空頻道");
        assert!(candidates(Band::Ghz5).contains(&suggested));
        assert_ne!(suggested, 36);
    }

    #[test]
    fn six_ghz_gets_no_channel_suggestion() {
        let c = analyze(&[
            own(Band::Ghz6, 37),
            ap(Band::Ghz6, 37, 80),
            ap(Band::Ghz6, 37, 70),
        ])
        .unwrap();
        assert_eq!(c.suggested_channel, None);
    }

    #[test]
    fn view_says_when_instability_and_crowding_do_not_line_up() {
        let crowded = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 6, 80),
            ap(Band::Ghz24, 6, 70),
        ])
        .unwrap();
        let clear = analyze(&[own(Band::Ghz24, 6)]).unwrap();

        let both = crowded.view(true).note.unwrap();
        assert!(both.contains("無法證明因果"), "{both}");

        let only_link = clear.view(true).note.unwrap();
        assert!(only_link.contains("並不擁擠"), "{only_link}");

        assert!(crowded.view(false).note.unwrap().contains("仍然穩定"));
        assert!(clear.view(false).note.is_none());
    }

    #[test]
    fn view_colors_follow_the_level_and_never_blame_a_neighbor_by_name() {
        let crowded = analyze(&[
            own(Band::Ghz24, 6),
            ap(Band::Ghz24, 6, 80),
            ap(Band::Ghz24, 6, 70),
        ])
        .unwrap();
        let v = crowded.view(false);
        assert_eq!((v.status_text.as_str(), v.color.as_str()), ("擁擠", "poor"));
        assert!(v.detail.contains("2.4 GHz"), "{}", v.detail);
        assert!(v.detail.contains("6 號頻道"), "{}", v.detail);
    }

    #[test]
    fn suggestions_go_to_it_staff_and_never_ask_users_to_touch_the_router() {
        for level_aps in [
            vec![
                own(Band::Ghz24, 6),
                ap(Band::Ghz24, 6, 80),
                ap(Band::Ghz24, 6, 70),
            ],
            vec![
                own(Band::Ghz5, 36),
                ap(Band::Ghz5, 36, 80),
                ap(Band::Ghz5, 36, 70),
            ],
            vec![
                own(Band::Ghz6, 37),
                ap(Band::Ghz6, 37, 80),
                ap(Band::Ghz6, 37, 70),
            ],
        ] {
            let v = analyze(&level_aps).unwrap().view(true);
            let text = v.suggestion.unwrap();
            assert!(text.contains("資訊人員"), "{text}");
            assert!(!text.contains("重開") && !text.contains("拔掉"), "{text}");
        }
    }

    #[test]
    fn waiting_and_unavailable_views_are_neutral() {
        let w = WifiEnvView::waiting();
        assert_eq!(w.color, "");
        let u = WifiEnvView::unavailable("原因");
        assert_eq!((u.status_text.as_str(), u.color.as_str()), ("無法分析", ""));
        assert_eq!(u.detail, "原因");
    }
}
