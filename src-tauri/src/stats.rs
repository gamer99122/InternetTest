//! 探測結果的統計計算與分級。
//!
//! 畫面上絕對不出現「抖動」「封包遺失率」這種字眼，但這裡照樣算完整指標，
//! 報告與 CSV 會保留原始數據，需要拿給電信商看的時候才有依據。

use std::collections::VecDeque;

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::targets::SegmentKind;

/// 單筆探測結果。`rtt_ms` 為 None 代表這次沒有回應（逾時或不可達）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Sample {
    pub at: DateTime<Local>,
    pub rtt_ms: Option<f64>,
}

/// 健康度分級。順序有意義：數字越大越糟，方便用 max() 取最差的一段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Grade {
    Good,
    Fair,
    Poor,
    Down,
}

impl Grade {
    /// 前端用來當 CSS class 的色票名稱。
    pub fn color(self) -> &'static str {
        match self {
            Grade::Good => "good",
            Grade::Fair => "fair",
            Grade::Poor => "poor",
            Grade::Down => "down",
        }
    }

    /// 單一段落的狀態文字。
    pub fn segment_text(self) -> &'static str {
        match self {
            Grade::Good => "正常",
            Grade::Fair => "有點慢",
            Grade::Poor => "不穩定",
            Grade::Down => "不通",
        }
    }
}

/// 各段落的評分門檻。不同段落的合理延遲差很多：
/// 連到家裡路由器 45ms 代表壞掉了，連到國外 45ms 卻是很好的成績。
#[derive(Debug, Clone, Copy)]
struct Thresholds {
    /// (良好上限, 尚可上限) 的延遲，單位 ms
    latency: (f64, f64),
    /// (良好上限, 尚可上限) 的抖動，單位 ms
    jitter: (f64, f64),
    /// (良好上限, 尚可上限) 的丟包率，單位 %
    loss: (f64, f64),
}

impl Thresholds {
    fn for_kind(kind: SegmentKind) -> Self {
        match kind {
            // 區域網路內，延遲應該是個位數
            SegmentKind::Router => Thresholds {
                latency: (10.0, 30.0),
                jitter: (5.0, 15.0),
                loss: (0.0, 2.0),
            },
            SegmentKind::Isp => Thresholds {
                latency: (40.0, 90.0),
                jitter: (15.0, 40.0),
                loss: (1.0, 4.0),
            },
            SegmentKind::Domestic => Thresholds {
                latency: (60.0, 120.0),
                jitter: (20.0, 50.0),
                loss: (1.0, 4.0),
            },
            // 國際線路本來就比較慢，門檻放寬
            SegmentKind::International => Thresholds {
                latency: (150.0, 280.0),
                jitter: (30.0, 70.0),
                loss: (2.0, 6.0),
            },
        }
    }
}

/// 一個滑動視窗內的統計摘要。
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub sent: usize,
    pub received: usize,
    pub loss_pct: f64,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub last_ms: Option<f64>,
    pub grade: Grade,
    /// 連續失敗次數，用來判斷「正在斷線中」
    pub consecutive_failures: usize,
}

impl Summary {
    /// 還沒有任何資料時的預設值。
    pub fn empty() -> Self {
        Summary {
            sent: 0,
            received: 0,
            loss_pct: 0.0,
            min_ms: None,
            avg_ms: None,
            max_ms: None,
            p95_ms: None,
            jitter_ms: None,
            last_ms: None,
            grade: Grade::Good,
            consecutive_failures: 0,
        }
    }
}

/// 固定容量的滑動視窗，只保留最近 N 筆探測結果。
#[derive(Debug)]
pub struct Window {
    samples: VecDeque<Sample>,
    capacity: usize,
    kind: SegmentKind,
    /// 從監測開始的累計值，不受滑動視窗長度影響
    total_sent: u64,
    total_received: u64,
}

impl Window {
    pub fn new(kind: SegmentKind, capacity: usize) -> Self {
        Window {
            samples: VecDeque::with_capacity(capacity),
            capacity,
            kind,
            total_sent: 0,
            total_received: 0,
        }
    }

    pub fn push(&mut self, sample: Sample) {
        self.total_sent += 1;
        if sample.rtt_ms.is_some() {
            self.total_received += 1;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    /// 累計可用率（%），涵蓋整段監測期間而非只有滑動視窗。
    pub fn lifetime_uptime_pct(&self) -> f64 {
        if self.total_sent == 0 {
            return 100.0;
        }
        self.total_received as f64 / self.total_sent as f64 * 100.0
    }

    /// 最近 `n` 筆延遲值，供前端畫折線圖。失敗的點是 None，圖上會斷開。
    pub fn recent_rtts(&self, n: usize) -> Vec<Option<f64>> {
        let skip = self.samples.len().saturating_sub(n);
        self.samples.iter().skip(skip).map(|s| s.rtt_ms).collect()
    }

    pub fn summarize(&self) -> Summary {
        if self.samples.is_empty() {
            return Summary::empty();
        }

        let sent = self.samples.len();
        let mut rtts: Vec<f64> = self.samples.iter().filter_map(|s| s.rtt_ms).collect();
        let received = rtts.len();
        let loss_pct = (sent - received) as f64 / sent as f64 * 100.0;

        // 連續失敗次數：從最新的一筆往回數
        let consecutive_failures = self
            .samples
            .iter()
            .rev()
            .take_while(|s| s.rtt_ms.is_none())
            .count();

        let last_ms = self.samples.back().and_then(|s| s.rtt_ms);

        if rtts.is_empty() {
            return Summary {
                sent,
                received: 0,
                loss_pct: 100.0,
                min_ms: None,
                avg_ms: None,
                max_ms: None,
                p95_ms: None,
                jitter_ms: None,
                last_ms: None,
                grade: Grade::Down,
                consecutive_failures,
            };
        }

        // 抖動要在排序前算，順序有意義
        let jitter_ms = self.jitter();

        let sum: f64 = rtts.iter().sum();
        let avg = sum / received as f64;

        rtts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let min = rtts[0];
        let max = rtts[received - 1];
        // 最近鄰法取 P95，樣本很少時也不會越界
        let p95_idx = ((received as f64 * 0.95).ceil() as usize).saturating_sub(1);
        let p95 = rtts[p95_idx.min(received - 1)];

        let grade = self.grade(loss_pct, avg, jitter_ms, consecutive_failures);

        Summary {
            sent,
            received,
            loss_pct,
            min_ms: Some(min),
            avg_ms: Some(avg),
            max_ms: Some(max),
            p95_ms: Some(p95),
            jitter_ms,
            last_ms,
            grade,
            consecutive_failures,
        }
    }

    /// 抖動：相鄰兩次成功探測的延遲差的平均絕對值（RFC 3550 的簡化版）。
    /// 中間夾著失敗的樣本不計入，避免把丟包誤算成抖動。
    fn jitter(&self) -> Option<f64> {
        let mut total = 0.0;
        let mut pairs = 0usize;
        let mut prev: Option<f64> = None;

        for s in &self.samples {
            match s.rtt_ms {
                Some(rtt) => {
                    if let Some(p) = prev {
                        total += (rtt - p).abs();
                        pairs += 1;
                    }
                    prev = Some(rtt);
                }
                // 探測失敗會打斷連續性
                None => prev = None,
            }
        }

        if pairs == 0 {
            None
        } else {
            Some(total / pairs as f64)
        }
    }

    fn grade(
        &self,
        loss_pct: f64,
        avg: f64,
        jitter: Option<f64>,
        consecutive_failures: usize,
    ) -> Grade {
        // 連續 3 次沒回應就當作這一段斷了，不必等統計數字慢慢變難看
        if consecutive_failures >= 3 {
            return Grade::Down;
        }

        let t = Thresholds::for_kind(self.kind);
        let j = jitter.unwrap_or(0.0);

        // 三個指標裡最糟的那個決定分級
        [
            level(loss_pct, t.loss),
            level(avg, t.latency),
            level(j, t.jitter),
        ]
        .into_iter()
        .max()
        .unwrap_or(Grade::Good)
    }
}

/// 把一個數值對照 (良好上限, 尚可上限) 換成分級。
fn level(value: f64, bounds: (f64, f64)) -> Grade {
    if value <= bounds.0 {
        Grade::Good
    } else if value <= bounds.1 {
        Grade::Fair
    } else {
        Grade::Poor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rtt: Option<f64>) -> Sample {
        Sample {
            at: Local::now(),
            rtt_ms: rtt,
        }
    }

    #[test]
    fn empty_window_is_good() {
        let w = Window::new(SegmentKind::Router, 10);
        assert_eq!(w.summarize().grade, Grade::Good);
    }

    #[test]
    fn jitter_ignores_gaps() {
        let mut w = Window::new(SegmentKind::Router, 10);
        // 10 -> 12 差 2；中間斷一次；20 -> 21 差 1。平均應該是 1.5
        for r in [Some(10.0), Some(12.0), None, Some(20.0), Some(21.0)] {
            w.push(sample(r));
        }
        let j = w.summarize().jitter_ms.unwrap();
        assert!((j - 1.5).abs() < 1e-9, "jitter = {j}");
    }

    #[test]
    fn three_failures_means_down() {
        let mut w = Window::new(SegmentKind::International, 10);
        w.push(sample(Some(20.0)));
        for _ in 0..3 {
            w.push(sample(None));
        }
        assert_eq!(w.summarize().grade, Grade::Down);
    }

    #[test]
    fn lan_latency_judged_harder_than_international() {
        let mut lan = Window::new(SegmentKind::Router, 10);
        let mut intl = Window::new(SegmentKind::International, 10);
        for _ in 0..5 {
            lan.push(sample(Some(45.0)));
            intl.push(sample(Some(45.0)));
        }
        // 同樣 45ms：對路由器來說是異常，對國際線路來說很好
        assert_eq!(lan.summarize().grade, Grade::Poor);
        assert_eq!(intl.summarize().grade, Grade::Good);
    }

    #[test]
    fn loss_pushes_grade_down() {
        let mut w = Window::new(SegmentKind::Domestic, 20);
        for i in 0..20 {
            // 每 4 筆掉 1 筆 = 25% 丟包
            w.push(sample(if i % 4 == 0 { None } else { Some(30.0) }));
        }
        let s = w.summarize();
        assert!((s.loss_pct - 25.0).abs() < 1e-9);
        assert_eq!(s.grade, Grade::Poor);
    }

    #[test]
    fn p95_does_not_overflow_on_small_samples() {
        let mut w = Window::new(SegmentKind::Router, 10);
        w.push(sample(Some(5.0)));
        assert_eq!(w.summarize().p95_ms, Some(5.0));
    }

    #[test]
    fn window_evicts_beyond_capacity_but_keeps_lifetime_totals() {
        let mut w = Window::new(SegmentKind::Router, 3);
        for _ in 0..10 {
            w.push(sample(Some(5.0)));
        }
        w.push(sample(None));
        // 視窗只留 3 筆，累計卻是 11 筆
        assert_eq!(w.summarize().sent, 3);
        assert_eq!(w.total_sent, 11);
        assert!((w.lifetime_uptime_pct() - 10.0 / 11.0 * 100.0).abs() < 1e-9);
    }
}
