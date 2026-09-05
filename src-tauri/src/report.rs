//! 產生可以直接寄給電信商客服的 HTML 報告。
//!
//! 分成兩半：上半部是白話結論，使用者自己看得懂；
//! 下半部是原始數據，客服的工程師看得懂。兩邊都需要。

use std::fmt::Write as _;

use chrono::Local;

use crate::monitor::{human_duration, Snapshot};

/// 把字串放進 HTML 之前先跳脫。
///
/// Wi-Fi 名稱、反查到的主機名稱都是外部來源，直接塞進 HTML 會壞掉。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 把毫秒數格式化成好讀的字串。
fn ms(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v:.0} 毫秒"),
        None => "—".to_string(),
    }
}

/// 產生完整的 HTML 報告。
pub fn render(snap: &Snapshot) -> String {
    let now = Local::now();
    let mut html = String::with_capacity(16 * 1024);

    let grade_class = snap.verdict.grade.color();

    html.push_str(
        r#"<!doctype html>
<html lang="zh-Hant">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>網路健檢報告</title>
<style>
  :root {
    --good: #16a34a; --fair: #d97706; --poor: #dc2626; --down: #991b1b;
    --ink: #1f2937; --muted: #6b7280; --line: #e5e7eb; --bg: #f9fafb;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; padding: 32px 20px;
    font-family: "Microsoft JhengHei", "PingFang TC", system-ui, sans-serif;
    color: var(--ink); background: var(--bg); line-height: 1.7;
  }
  .sheet { max-width: 820px; margin: 0 auto; background: #fff;
           border: 1px solid var(--line); border-radius: 14px; padding: 36px; }
  h1 { font-size: 26px; margin: 0 0 4px; }
  h2 { font-size: 18px; margin: 34px 0 12px; padding-bottom: 8px;
       border-bottom: 2px solid var(--line); }
  .meta { color: var(--muted); font-size: 14px; margin-bottom: 26px; }
  .verdict { border-radius: 12px; padding: 20px 22px; margin-bottom: 8px;
             border-left: 6px solid var(--muted); background: var(--bg); }
  .verdict.good { border-color: var(--good); }
  .verdict.fair { border-color: var(--fair); }
  .verdict.poor, .verdict.down { border-color: var(--poor); }
  .verdict h3 { margin: 0 0 8px; font-size: 21px; }
  .verdict p { margin: 0 0 6px; }
  .advice { margin-top: 12px; padding: 12px 14px; background: #fff;
            border: 1px dashed var(--line); border-radius: 8px; font-size: 15px; }
  .cards { display: grid; grid-template-columns: repeat(2, 1fr); gap: 12px; }
  .card { border: 1px solid var(--line); border-radius: 10px; padding: 14px 16px; }
  .card .name { font-weight: 700; margin-bottom: 2px; }
  .card .desc { color: var(--muted); font-size: 13px; margin-bottom: 10px; }
  .pill { display: inline-block; padding: 2px 10px; border-radius: 999px;
          font-size: 13px; font-weight: 700; color: #fff; }
  .pill.good { background: var(--good); } .pill.fair { background: var(--fair); }
  .pill.poor { background: var(--poor); } .pill.down { background: var(--down); }
  table { width: 100%; border-collapse: collapse; font-size: 14px; }
  th, td { text-align: left; padding: 9px 10px; border-bottom: 1px solid var(--line); }
  th { background: var(--bg); font-weight: 700; }
  td.num { text-align: right; font-variant-numeric: tabular-nums; }
  .empty { color: var(--muted); font-style: italic; }
  .tech { font-size: 13px; color: var(--muted); }
  .tech table { font-family: Consolas, "Courier New", monospace; font-size: 12.5px; }
  footer { margin-top: 34px; padding-top: 14px; border-top: 1px solid var(--line);
           color: var(--muted); font-size: 12.5px; }
  @media print { body { background: #fff; padding: 0; }
                 .sheet { border: none; padding: 0; } }
</style>
</head>
<body>
<div class="sheet">
"#,
    );

    // --- 標題 ---
    let _ = write!(
        html,
        r#"<h1>網路健檢報告</h1>
<div class="meta">產生時間：{}　|　檢測時長：{}　|　使用的網路：{}</div>
"#,
        now.format("%Y 年 %-m 月 %-d 日 %H:%M"),
        esc(&snap.elapsed_text.replace("已檢測 ", "")),
        esc(&snap.interface_name),
    );

    // --- 結論 ---
    let _ = write!(
        html,
        r#"<div class="verdict {}">
  <h3>{}</h3>
  <p>{}</p>
"#,
        grade_class,
        esc(&snap.verdict.headline),
        esc(&snap.verdict.explanation),
    );
    if let Some(s) = &snap.verdict.suggestion {
        let _ = write!(html, r#"  <div class="advice">建議：{}</div>"#, esc(s));
    }
    html.push_str("</div>\n");

    // --- 摘要數字 ---
    let _ = write!(
        html,
        r#"<h2>整體狀況</h2>
<table>
  <tr><th>連線穩定度</th><td>{:.1}%</td></tr>
  <tr><th>斷線情形</th><td>{}</td></tr>
  <tr><th>查詢網址速度</th><td>{}（{}）</td></tr>
"#,
        snap.stability_pct,
        esc(&snap.outage_text),
        esc(&snap.dns.status_text),
        ms(snap.dns.avg_ms),
    );
    if let Some(w) = &snap.wifi_text {
        let _ = write!(html, "  <tr><th>無線訊號</th><td>{}</td></tr>\n", esc(w));
    }
    if let Some(isp) = &snap.isp_name {
        let _ = write!(html, "  <tr><th>網路業者</th><td>{}</td></tr>\n", esc(isp));
    }
    html.push_str("</table>\n");

    // --- 四段狀態 ---
    html.push_str("<h2>各段連線狀況</h2>\n<div class=\"cards\">\n");
    for seg in &snap.segments {
        let _ = write!(
            html,
            r#"  <div class="card">
    <div class="name">{}</div>
    <div class="desc">{}</div>
    <span class="pill {}">{}</span>
    <div style="margin-top:8px;font-size:13.5px">平均反應 {}</div>
  </div>
"#,
            esc(&seg.label),
            esc(&seg.description),
            seg.color,
            esc(&seg.status_text),
            ms(seg.avg_ms),
        );
    }
    html.push_str("</div>\n");

    // --- 斷線紀錄 ---
    html.push_str("<h2>斷線紀錄</h2>\n");
    if snap.events.is_empty() {
        html.push_str("<p class=\"empty\">整段檢測期間都沒有斷線。</p>\n");
    } else {
        html.push_str(
            "<table>\n<tr><th>發生時間</th><th>持續多久</th><th>說明</th></tr>\n",
        );
        for e in &snap.events {
            let _ = write!(
                html,
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                esc(&e.started_text),
                match e.duration_secs {
                    Some(s) => esc(&human_duration(s)),
                    None => "仍在斷線中".to_string(),
                },
                esc(&e.text),
            );
        }
        html.push_str("</table>\n");
    }

    // --- 技術附錄 ---
    html.push_str(
        "<h2>技術數據</h2>\n<p class=\"tech\">以下提供給網路業者的技術人員參考。</p>\n",
    );
    html.push_str(
        "<div class=\"tech\">\n<table>\n<tr><th>段落</th><th>探測目標</th><th>平均</th><th>最高</th><th>抖動</th><th>封包遺失</th></tr>\n",
    );
    for seg in &snap.segments {
        let _ = write!(
            html,
            "<tr><td>{}</td><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{:.1}%</td></tr>\n",
            esc(&seg.label),
            esc(&seg.target),
            ms(seg.avg_ms),
            ms(seg.max_ms),
            ms(seg.jitter_ms),
            seg.loss_pct,
        );
    }
    html.push_str("</table>\n");

    // 斷線當下的路徑追蹤，最有診斷價值的部分
    let traces: Vec<_> = snap
        .events
        .iter()
        .filter_map(|e| e.trace.as_ref().map(|t| (e, t)))
        .collect();
    if !traces.is_empty() {
        html.push_str("<h3 style=\"font-size:15px;margin-top:22px\">斷線當下的連線路徑</h3>\n");
        for (event, hops) in traces.iter().take(5) {
            let _ = write!(
                html,
                "<p style=\"margin:14px 0 6px\"><strong>{}</strong> 的路徑追蹤：</p>\n<table>\n<tr><th>#</th><th>位址</th><th>主機名稱</th><th>反應時間</th><th>回應</th></tr>\n",
                esc(&event.started_text)
            );
            for hop in hops.iter() {
                let _ = write!(
                    html,
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"num\">{}</td><td class=\"num\">{}/{}</td></tr>\n",
                    hop.ttl,
                    hop.addr.map(|a| a.to_string()).unwrap_or_else(|| "*".into()),
                    esc(hop.hostname.as_deref().unwrap_or("—")),
                    ms(hop.rtt_ms),
                    hop.received,
                    hop.sent,
                );
            }
            html.push_str("</table>\n");
        }
    }
    html.push_str("</div>\n");

    if !snap.notes.is_empty() {
        html.push_str("<h2>檢測備註</h2>\n<ul class=\"tech\">\n");
        for note in &snap.notes {
            let _ = write!(html, "<li>{}</li>\n", esc(note));
        }
        html.push_str("</ul>\n");
    }

    let _ = write!(
        html,
        r#"<footer>本報告由「網路健檢」自動產生。逐筆原始紀錄另存於 {}。</footer>
</div>
</body>
</html>
"#,
        esc(snap.log_path.as_deref().unwrap_or("（未儲存）")),
    );

    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_special_characters() {
        assert_eq!(esc("a<b>c"), "a&lt;b&gt;c");
        assert_eq!(esc("Tom & Jerry"), "Tom &amp; Jerry");
        assert_eq!(esc(r#""quoted""#), "&quot;quoted&quot;");
        // 中文不受影響
        assert_eq!(esc("中華電信"), "中華電信");
    }

    #[test]
    fn malicious_ssid_cannot_inject_script() {
        let evil = "<script>alert(1)</script>";
        let escaped = esc(evil);
        assert!(!escaped.contains("<script>"));
        assert!(escaped.contains("&lt;script&gt;"));
    }

    #[test]
    fn formats_missing_values_as_dash() {
        assert_eq!(ms(None), "—");
        assert_eq!(ms(Some(23.4)), "23 毫秒");
    }
}
