//! 把每一次探測寫進 CSV。
//!
//! 畫面上給的是白話結論，但真的要跟電信商理論的時候需要原始數據。
//! 每一列就是一次探測，用 Excel 打開就能自己畫圖。

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};

use crate::targets::SegmentKind;

/// 逐筆探測紀錄的 CSV 寫入器。
pub struct Logger {
    writer: csv::Writer<File>,
    path: PathBuf,
    rows: u64,
}

impl Logger {
    /// 在 `dir` 底下開一個以開始時間命名的 CSV。
    pub fn create(dir: &Path, started: DateTime<Local>) -> anyhow::Result<Self> {
        fs::create_dir_all(dir)?;
        let name = format!("紀錄_{}.csv", started.format("%Y-%m-%d_%H%M%S"));
        let path = dir.join(name);

        let file = File::create(&path)?;
        let mut writer = csv::Writer::from_writer(file);

        // Excel 預設用系統編碼開 CSV，中文標題會變亂碼。
        // 這裡靠標題列全用英文避開這個問題，中文只出現在報告的 HTML 裡。
        writer.write_record([
            "timestamp",
            "segment",
            "target",
            "rtt_ms",
            "success",
        ])?;
        writer.flush()?;

        Ok(Logger {
            writer,
            path,
            rows: 0,
        })
    }

    pub fn log(
        &mut self,
        at: DateTime<Local>,
        kind: SegmentKind,
        target: &str,
        rtt_ms: Option<f64>,
    ) -> anyhow::Result<()> {
        self.writer.write_record([
            at.format("%Y-%m-%d %H:%M:%S%.3f").to_string().as_str(),
            segment_code(kind),
            target,
            &rtt_ms.map(|v| format!("{v:.3}")).unwrap_or_default(),
            if rtt_ms.is_some() { "1" } else { "0" },
        ])?;
        self.rows += 1;

        // 每 20 列同步一次。斷電或當掉時最多只會少掉幾秒的資料，
        // 又不會每秒都在硬碟上寫小檔案。
        if self.rows % 20 == 0 {
            self.writer.flush()?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> anyhow::Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// CSV 裡用的段落代碼，固定英文才不會有編碼問題。
fn segment_code(kind: SegmentKind) -> &'static str {
    match kind {
        SegmentKind::Router => "router",
        SegmentKind::Isp => "isp",
        SegmentKind::Domestic => "domestic",
        SegmentKind::International => "international",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_header_and_rows() {
        let dir = std::env::temp_dir().join("internettest-logger-test");
        let _ = fs::remove_dir_all(&dir);

        let now = Local::now();
        let mut logger = Logger::create(&dir, now).expect("建立 CSV");
        logger
            .log(now, SegmentKind::Router, "192.168.1.1", Some(3.25))
            .unwrap();
        logger
            .log(now, SegmentKind::International, "1.1.1.1", None)
            .unwrap();
        logger.flush().unwrap();

        let content = fs::read_to_string(logger.path()).unwrap();
        let lines: Vec<&str> = content.lines().collect();

        assert_eq!(lines[0], "timestamp,segment,target,rtt_ms,success");
        assert!(lines[1].contains("router,192.168.1.1,3.250,1"), "{}", lines[1]);
        // 失敗的那筆 rtt 欄位留空，success 是 0
        assert!(lines[2].ends_with("international,1.1.1.1,,0"), "{}", lines[2]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn filename_contains_the_start_time() {
        let dir = std::env::temp_dir().join("internettest-logger-name-test");
        let _ = fs::remove_dir_all(&dir);

        let now = Local::now();
        let logger = Logger::create(&dir, now).expect("建立 CSV");
        let name = logger.path().file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("紀錄_"), "{name}");
        assert!(name.ends_with(".csv"), "{name}");
        assert!(name.contains(&now.format("%Y-%m-%d").to_string()), "{name}");

        let _ = fs::remove_dir_all(&dir);
    }
}
