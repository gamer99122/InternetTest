//! 把四段的量測結果變成一句人話。
//!
//! 這是整個工具存在的理由。使用者不想知道「第 3 跳丟包 12%」，
//! 他想知道的是「這是我家的問題還是電信公司的問題，我該打電話給誰」。

use serde::Serialize;

use crate::stats::{Grade, Summary};
use crate::targets::SegmentKind;
use crate::wifi::WifiStatus;

/// 一段路徑目前的狀態，`diagnose` 的輸入。
pub struct Reading<'a> {
    pub kind: SegmentKind,
    /// 這一段的顯示名稱，例如「中華電信」
    pub label: &'a str,
    pub summary: &'a Summary,
}

/// DNS 的健康狀態。
#[derive(Debug, Clone, Copy, Default)]
pub struct DnsHealth {
    /// 最近一次解析成功了嗎
    pub working: bool,
    /// 平均解析耗時
    pub avg_ms: Option<f64>,
}

impl DnsHealth {
    /// 超過 300ms 使用者就會感覺到「按下去要等一下才有反應」。
    fn is_slow(&self) -> bool {
        self.avg_ms.is_some_and(|ms| ms > 300.0)
    }
}

/// 一次診斷的結論。
#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub grade: Grade,
    /// 主標題，例如「網路狀況良好」
    pub headline: String,
    /// 說明問題出在哪一段
    pub explanation: String,
    /// 建議使用者做什麼；沒事的時候是 None
    pub suggestion: Option<String>,
    /// 判定的問題來源，前端拿它把對應的卡片標紅
    pub culprit: Option<SegmentKind>,
}

/// 幾分算「這一段有問題」。Fair（有點慢）不足以構成歸咎的理由，
/// 否則使用者會一直看到誤報。
fn is_broken(grade: Grade) -> bool {
    grade >= Grade::Poor
}

/// 產生診斷結論。
///
/// 歸因原則：**怪最近的那一段**。網路是串連的，遠端那一段壞掉時，
/// 經過它的所有更遠段落也會一起壞；真正的斷點是「第一個壞掉的段落」。
pub fn diagnose(readings: &[Reading<'_>], wifi: Option<&WifiStatus>, dns: DnsHealth) -> Verdict {
    if readings.is_empty() {
        return Verdict {
            grade: Grade::Good,
            headline: "尚未開始檢測".into(),
            explanation: "按下「開始檢測」就會開始檢查你的網路。".into(),
            suggestion: None,
            culprit: None,
        };
    }

    // 由近到遠排序，才能找出第一個斷點
    let mut sorted: Vec<&Reading<'_>> = readings.iter().collect();
    sorted.sort_by_key(|r| r.kind);

    let all_down = sorted.iter().all(|r| r.summary.grade == Grade::Down);
    let worst = sorted
        .iter()
        .map(|r| r.summary.grade)
        .max()
        .unwrap_or(Grade::Good);

    // --- 全斷：通常是實體連線的問題，不是網路壞掉 ---
    if all_down {
        let explanation = match wifi {
            Some(w) if w.signal_pct.is_some() => {
                format!(
                    "本次測試目標都未回應。{}；仍需確認 ICMP 政策。",
                    w.plain_text().unwrap_or_else(|| "有偵測到 Wi-Fi".into())
                )
            }
            _ => "本次測試目標都未回應，可能是連線異常或 ICMP 被限制。".into(),
        };
        return Verdict {
            grade: Grade::Down,
            headline: "測試目標未回應".into(),
            explanation,
            suggestion: Some(
                "請確認網路線及 Wi-Fi 連線，並聯絡機關資訊人員確認是否限制測試流量。".into(),
            ),
            culprit: Some(SegmentKind::Router),
        };
    }

    // --- 找出第一個壞掉的段落 ---
    if let Some(bad) = sorted.iter().find(|r| is_broken(r.summary.grade)) {
        let kind = bad.kind;
        let mut explanation = String::new();

        // 家裡這一段壞掉時，如果訊號也很弱，那八成就是訊號的問題
        if kind == SegmentKind::Router {
            if let Some(w) = wifi.filter(|w| w.is_weak()) {
                return Verdict {
                    grade: bad.summary.grade,
                    headline: "Wi-Fi 訊號太弱".into(),
                    explanation: format!(
                        "{}，這就是網路會卡的原因。這不是網路公司的問題。",
                        w.plain_text().unwrap_or_else(|| "Wi-Fi 訊號很弱".into())
                    ),
                    suggestion: Some("請靠近無線分享器一點，或是改用網路線直接連接。".into()),
                    culprit: Some(SegmentKind::Router),
                };
            }
        }

        // 標準歸因：講清楚哪一段好、哪一段壞
        let healthy_before: Vec<&str> = sorted
            .iter()
            .take_while(|r| r.kind != kind)
            .map(|r| r.label)
            .collect();

        if !healthy_before.is_empty() {
            explanation.push_str(&format!("{}都正常，", healthy_before.join("、")));
        }
        explanation.push_str(&format!("但{}這一段有問題。", bad.label));
        explanation.push_str(kind.blame());
        explanation.push('。');

        return Verdict {
            grade: bad.summary.grade,
            headline: describe_symptom(bad.summary),
            explanation,
            suggestion: Some(kind.suggestion().to_string()),
            culprit: Some(kind),
        };
    }

    // --- 連線都正常，但 DNS 有問題 ---
    // 這一類問題最容易被誤判成「網路很慢」，其實線路完全沒事
    if !dns.working {
        return Verdict {
            grade: Grade::Poor,
            headline: "網路通，但打不開網頁".into(),
            explanation: "連線本身是正常的，但是查詢網址的服務（DNS）沒有回應，所以網頁會打不開。"
                .into(),
            suggestion: Some("請由機關資訊人員確認核准的 DNS 設定與查詢政策。".into()),
            culprit: None,
        };
    }

    if dns.is_slow() {
        return Verdict {
            grade: Grade::Fair,
            headline: "開網頁前會頓一下".into(),
            explanation: format!(
                "連線速度正常，但查詢網址平均要花 {:.0} 毫秒，所以每次打開新網頁都會先卡一下。",
                dns.avg_ms.unwrap_or_default()
            ),
            suggestion: Some("請由機關資訊人員檢查 DNS 回應時間。".into()),
            culprit: None,
        };
    }

    // --- 一切正常，或只是稍微慢一點 ---
    match worst {
        Grade::Good => Verdict {
            grade: Grade::Good,
            headline: "網路狀況良好".into(),
            explanation: "本次啟用的測試目標回應穩定；未測項目與實際應用服務仍需另行確認。".into(),
            suggestion: None,
            culprit: None,
        },
        _ => {
            let slow = sorted
                .iter()
                .find(|r| r.summary.grade == Grade::Fair)
                .map(|r| r.label)
                .unwrap_or("某一段");
            Verdict {
                grade: Grade::Fair,
                headline: "網路大致正常".into(),
                explanation: format!(
                    "網路可以正常使用，只是{slow}比平常慢一些，一般上網不太會感覺得到。"
                ),
                suggestion: None,
                culprit: None,
            }
        }
    }
}

/// 根據數據挑一句最貼近使用者體感的標題。
///
/// 同樣是「不穩定」，一直斷線和延遲忽高忽低，感受完全不一樣。
fn describe_symptom(s: &Summary) -> String {
    if s.grade == Grade::Down {
        return "網路斷線中".into();
    }
    if s.loss_pct > 5.0 {
        return "網路會斷斷續續".into();
    }
    if s.jitter_ms.is_some_and(|j| j > 50.0) {
        return "網路忽快忽慢".into();
    }
    if s.avg_ms.is_some_and(|a| a > 200.0) {
        return "網路反應很慢".into();
    }
    "網路不太穩定".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(grade: Grade, loss: f64, avg: f64, jitter: f64) -> Summary {
        Summary {
            sent: 30,
            received: 30,
            loss_pct: loss,
            min_ms: Some(avg),
            avg_ms: Some(avg),
            max_ms: Some(avg),
            p95_ms: Some(avg),
            jitter_ms: Some(jitter),
            last_ms: Some(avg),
            grade,
            consecutive_failures: if grade == Grade::Down { 3 } else { 0 },
        }
    }

    fn good() -> Summary {
        summary(Grade::Good, 0.0, 5.0, 1.0)
    }

    fn healthy_dns() -> DnsHealth {
        DnsHealth {
            working: true,
            avg_ms: Some(30.0),
        }
    }

    #[test]
    fn everything_healthy_blames_nobody() {
        let (r, i, d, n) = (good(), good(), good(), good());
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &r,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "中華電信",
                summary: &i,
            },
            Reading {
                kind: SegmentKind::Domestic,
                label: "國內網路",
                summary: &d,
            },
            Reading {
                kind: SegmentKind::International,
                label: "國外網路",
                summary: &n,
            },
        ];
        let v = diagnose(&readings, None, healthy_dns());
        assert_eq!(v.grade, Grade::Good);
        assert!(v.culprit.is_none());
        assert!(v.suggestion.is_none());
    }

    #[test]
    fn blames_isp_when_home_is_fine() {
        // 這是最重要的一個案例：家裡正常、電信商那段壞掉
        let r = good();
        let bad = summary(Grade::Poor, 8.0, 120.0, 60.0);
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &r,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "中華電信",
                summary: &bad,
            },
            Reading {
                kind: SegmentKind::Domestic,
                label: "國內網路",
                summary: &bad,
            },
            Reading {
                kind: SegmentKind::International,
                label: "國外網路",
                summary: &bad,
            },
        ];
        let v = diagnose(&readings, None, healthy_dns());
        assert_eq!(v.culprit, Some(SegmentKind::Isp));
        assert!(v.explanation.contains("中華電信"), "{}", v.explanation);
        assert!(
            v.explanation.contains("家裡的網路都正常"),
            "{}",
            v.explanation
        );
        assert!(v.suggestion.unwrap().contains("資訊人員"));
    }

    #[test]
    fn blames_nearest_broken_segment_not_the_worst_one() {
        // 家裡就壞了，後面全部跟著壞。斷點是家裡，不是國外。
        let bad = summary(Grade::Poor, 10.0, 90.0, 40.0);
        let worse = summary(Grade::Down, 100.0, 0.0, 0.0);
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &bad,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "中華電信",
                summary: &worse,
            },
            Reading {
                kind: SegmentKind::Domestic,
                label: "國內網路",
                summary: &worse,
            },
        ];
        let v = diagnose(&readings, None, healthy_dns());
        assert_eq!(v.culprit, Some(SegmentKind::Router));
    }

    #[test]
    fn weak_wifi_gets_its_own_diagnosis() {
        let bad = summary(Grade::Poor, 6.0, 40.0, 25.0);
        let g = good();
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &bad,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "中華電信",
                summary: &g,
            },
        ];
        let weak = WifiStatus {
            ssid: Some("MyWiFi".into()),
            signal_pct: Some(22),
            rx_mbps: Some(6),
        };
        let v = diagnose(&readings, Some(&weak), healthy_dns());
        assert_eq!(v.headline, "Wi-Fi 訊號太弱");
        assert!(v.suggestion.unwrap().contains("靠近"));
    }

    #[test]
    fn international_only_problem_says_domestic_is_fine() {
        let g = good();
        let bad = summary(Grade::Poor, 7.0, 320.0, 90.0);
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &g,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "中華電信",
                summary: &g,
            },
            Reading {
                kind: SegmentKind::Domestic,
                label: "國內網路",
                summary: &g,
            },
            Reading {
                kind: SegmentKind::International,
                label: "國外網路",
                summary: &bad,
            },
        ];
        let v = diagnose(&readings, None, healthy_dns());
        assert_eq!(v.culprit, Some(SegmentKind::International));
        assert!(
            v.explanation.contains("國內網路都正常"),
            "{}",
            v.explanation
        );
    }

    #[test]
    fn everything_down_suggests_checking_the_cable() {
        let down = summary(Grade::Down, 100.0, 0.0, 0.0);
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &down,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "電信商",
                summary: &down,
            },
        ];
        let v = diagnose(&readings, None, DnsHealth::default());
        assert_eq!(v.grade, Grade::Down);
        assert!(v.suggestion.unwrap().contains("網路線"));
    }

    #[test]
    fn broken_dns_is_reported_even_when_pings_are_perfect() {
        let g = good();
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &g,
            },
            Reading {
                kind: SegmentKind::International,
                label: "國外網路",
                summary: &g,
            },
        ];
        let v = diagnose(
            &readings,
            None,
            DnsHealth {
                working: false,
                avg_ms: None,
            },
        );
        assert_eq!(v.headline, "網路通，但打不開網頁");
    }

    #[test]
    fn slow_dns_downgrades_to_fair() {
        let g = good();
        let readings = vec![Reading {
            kind: SegmentKind::Router,
            label: "家裡的網路",
            summary: &g,
        }];
        let v = diagnose(
            &readings,
            None,
            DnsHealth {
                working: true,
                avg_ms: Some(900.0),
            },
        );
        assert_eq!(v.grade, Grade::Fair);
        assert!(v.explanation.contains("900"), "{}", v.explanation);
    }

    #[test]
    fn fair_segments_do_not_trigger_blame() {
        // 「有點慢」不該讓使用者以為壞掉了
        let fair = summary(Grade::Fair, 1.0, 50.0, 20.0);
        let readings = vec![Reading {
            kind: SegmentKind::Isp,
            label: "中華電信",
            summary: &fair,
        }];
        let v = diagnose(&readings, None, healthy_dns());
        assert!(v.culprit.is_none());
        assert_eq!(v.grade, Grade::Fair);
    }

    #[test]
    fn no_technical_jargon_in_user_facing_text() {
        let bad = summary(Grade::Poor, 9.0, 150.0, 70.0);
        let g = good();
        let readings = vec![
            Reading {
                kind: SegmentKind::Router,
                label: "家裡的網路",
                summary: &g,
            },
            Reading {
                kind: SegmentKind::Isp,
                label: "中華電信",
                summary: &bad,
            },
        ];
        let v = diagnose(&readings, None, healthy_dns());
        let text = format!(
            "{} {} {}",
            v.headline,
            v.explanation,
            v.suggestion.unwrap_or_default()
        );
        for jargon in ["jitter", "抖動", "封包", "ms", "packet", "RTT", "延遲率"] {
            assert!(!text.contains(jargon), "不該出現術語「{jargon}」：{text}");
        }
    }

    #[test]
    fn symptom_headline_matches_the_actual_problem() {
        assert_eq!(
            describe_symptom(&summary(Grade::Down, 100.0, 0.0, 0.0)),
            "網路斷線中"
        );
        assert_eq!(
            describe_symptom(&summary(Grade::Poor, 12.0, 30.0, 5.0)),
            "網路會斷斷續續"
        );
        assert_eq!(
            describe_symptom(&summary(Grade::Poor, 0.0, 30.0, 80.0)),
            "網路忽快忽慢"
        );
        assert_eq!(
            describe_symptom(&summary(Grade::Poor, 0.0, 400.0, 5.0)),
            "網路反應很慢"
        );
    }
}
