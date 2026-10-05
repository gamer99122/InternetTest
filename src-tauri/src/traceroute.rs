//! 逐跳路徑追蹤（traceroute）。
//!
//! 兩個用途：
//! 1. 啟動時找出「電信商的第一台機器」是誰，才有辦法把問題定位到 ISP 那一段。
//! 2. 偵測到斷線時自動跑一次，記錄下究竟是斷在哪一跳。
//!
//! 一樣是走 `IcmpSendEcho` 加 TTL，不需要系統管理員權限。

use std::net::Ipv4Addr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use serde::Serialize;

use crate::icmp::{Echo, IcmpHandle, Outcome};

/// 路徑上的一跳。
#[derive(Debug, Clone, Serialize)]
pub struct Hop {
    /// 距離幾跳（等同送出時用的 TTL）
    pub ttl: u8,
    pub addr: Option<Ipv4Addr>,
    /// 這一跳的平均延遲；沒有回應時是 None
    pub rtt_ms: Option<f64>,
    /// 反查到的主機名稱，用來判斷是哪一家電信商
    pub hostname: Option<String>,
    /// 這一跳送了幾次、回了幾次，用來標示部分丟包
    pub sent: u8,
    pub received: u8,
}

/// 判斷是不是 RFC1918 私有位址、link-local 或 loopback。
///
/// 注意 CGNAT（100.64.0.0/10）刻意不算在內 —— 那段位址是電信商自己的設備，
/// 對「問題出在哪一段」的判斷來說它屬於 ISP 那一側。
pub fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local() || ip.is_loopback() || ip.is_unspecified()
}

/// 追蹤到 `dest` 的路徑。
///
/// `probes_per_hop` 建議值：找 ISP 用 1（要快），斷線診斷用 3（要準）。
pub async fn trace(
    dest: Ipv4Addr,
    max_hops: u8,
    timeout: Duration,
    probes_per_hop: u8,
) -> anyhow::Result<Vec<Hop>> {
    trace_cancellable(dest, max_hops, timeout, probes_per_hop, None).await
}

pub async fn trace_cancellable(
    dest: Ipv4Addr,
    max_hops: u8,
    timeout: Duration,
    probes_per_hop: u8,
    cancel: Option<Arc<AtomicBool>>,
) -> anyhow::Result<Vec<Hop>> {
    // IcmpSendEcho 是阻塞呼叫，整段搬到 blocking 執行緒，不要卡住 tokio runtime
    let hops = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Hop>> {
        let handle = IcmpHandle::open()?;
        let mut hops = Vec::new();

        for ttl in 1..=max_hops {
            let mut addr = None;
            let mut total_ms = 0.0;
            let mut received = 0u8;
            let mut reached_destination = false;

            for _ in 0..probes_per_hop.max(1) {
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
                    return Ok(hops);
                }
                let echo: Echo = handle.echo(dest, timeout, Some(ttl));

                match echo.outcome {
                    Outcome::Reached => {
                        reached_destination = true;
                        addr = echo.from.or(Some(dest));
                        if let Some(rtt) = echo.rtt {
                            total_ms += rtt.as_secs_f64() * 1000.0;
                            received += 1;
                        }
                    }
                    Outcome::TtlExpired => {
                        addr = echo.from;
                        if let Some(rtt) = echo.rtt {
                            total_ms += rtt.as_secs_f64() * 1000.0;
                            received += 1;
                        }
                    }
                    // 不可達的路由器至少告訴我們它是誰，還是記下來
                    Outcome::Unreachable => {
                        addr = echo.from.or(addr);
                    }
                    Outcome::TimedOut | Outcome::Failed => {}
                }
            }

            hops.push(Hop {
                ttl,
                addr,
                rtt_ms: if received > 0 {
                    Some(total_ms / received as f64)
                } else {
                    None
                },
                hostname: None,
                sent: probes_per_hop.max(1),
                received,
            });

            if reached_destination {
                break;
            }
        }

        Ok(hops)
    })
    .await??;

    Ok(hops)
}

/// 對每一跳做反向 DNS 查詢，補上主機名稱。
///
/// 這一步純粹是為了讓報告好讀（看得出「這一跳是中華電信的機器」），
/// 查不到就算了，不影響任何判斷。`cancel` 被設定時立刻停止，已經查到的保留。
pub async fn resolve_hostnames(hops: &mut [Hop], cancel: &AtomicBool) {
    for hop in hops.iter_mut() {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let Some(addr) = hop.addr else { continue };
        // 私有位址不可能有公開的 PTR，省下這次查詢
        if is_private_v4(addr) {
            continue;
        }
        hop.hostname = crate::dns::reverse(addr).await;
    }
}

/// 從路徑中找出第一個屬於電信商的節點。
///
/// 判斷方式是「第一個非私有位址的跳」：家裡的路由器和多層 NAT 都是私有位址，
/// 一旦跳出私有網段，那台機器就是電信商的設備。
pub fn first_isp_hop(hops: &[Hop]) -> Option<&Hop> {
    hops.iter()
        .find(|h| h.addr.is_some_and(|a| !is_private_v4(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(ttl: u8, addr: Option<[u8; 4]>) -> Hop {
        Hop {
            ttl,
            addr: addr.map(|o| Ipv4Addr::from(o)),
            rtt_ms: Some(1.0),
            hostname: None,
            sent: 1,
            received: 1,
        }
    }

    #[test]
    fn private_ranges_are_recognised() {
        assert!(is_private_v4(Ipv4Addr::new(192, 168, 1, 1)));
        assert!(is_private_v4(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_private_v4(Ipv4Addr::new(172, 16, 0, 1)));
        assert!(is_private_v4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_private_v4(Ipv4Addr::new(169, 254, 1, 1)));

        assert!(!is_private_v4(Ipv4Addr::new(1, 1, 1, 1)));
        assert!(!is_private_v4(Ipv4Addr::new(168, 95, 1, 1)));
        // CGNAT 屬於電信商那一側，不能算成「家裡」
        assert!(!is_private_v4(Ipv4Addr::new(100, 64, 0, 1)));
    }

    #[test]
    fn isp_hop_is_first_public_address() {
        let hops = vec![
            hop(1, Some([192, 168, 1, 1])),
            hop(2, Some([10, 0, 0, 1])),
            hop(3, Some([168, 95, 98, 254])),
            hop(4, Some([1, 1, 1, 1])),
        ];
        assert_eq!(
            first_isp_hop(&hops).unwrap().addr,
            Some(Ipv4Addr::new(168, 95, 98, 254))
        );
    }

    #[test]
    fn isp_hop_skips_unanswered_hops() {
        let hops = vec![
            hop(1, Some([192, 168, 0, 1])),
            hop(2, None),
            hop(3, Some([100, 64, 5, 1])),
        ];
        // 中間沒回應的那一跳要跳過，CGNAT 那跳才是 ISP
        assert_eq!(
            first_isp_hop(&hops).unwrap().addr,
            Some(Ipv4Addr::new(100, 64, 5, 1))
        );
    }

    #[test]
    fn all_private_path_has_no_isp_hop() {
        let hops = vec![hop(1, Some([192, 168, 1, 1])), hop(2, None)];
        assert!(first_isp_hop(&hops).is_none());
    }
}
