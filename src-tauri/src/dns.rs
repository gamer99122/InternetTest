//! DNS 相關的探測。
//!
//! 很多「網路好慢」的體感其實是 DNS 解析慢，而不是連線本身有問題。
//! 只 ping IP 位址完全看不出這一類問題，所以這裡獨立量測解析耗時。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use hickory_resolver::proto::rr::RData;
use hickory_resolver::{Resolver, TokioResolver};

/// 一般用途的解析器（有快取），拿來做反向查詢。
static RESOLVER: OnceLock<Arc<TokioResolver>> = OnceLock::new();

/// 專門用來計時的解析器，關掉快取，否則第二次之後量到的都是 0ms。
static TIMING_RESOLVER: OnceLock<Arc<TokioResolver>> = OnceLock::new();

/// 輪流查詢這幾個網域，避免固定查同一個造成偏差。
const PROBE_DOMAINS: &[&str] = &[
    "www.google.com",
    "tw.yahoo.com",
    "www.youtube.com",
    "www.facebook.com",
    "www.pchome.com.tw",
];

/// 取得（或初始化）一般用途的解析器。
pub fn system_resolver() -> anyhow::Result<Arc<TokioResolver>> {
    if let Some(r) = RESOLVER.get() {
        return Ok(r.clone());
    }
    let resolver = Arc::new(Resolver::builder_tokio()?.build()?);
    // 併發初始化時可能已經有別人設過了，用最後生效的那個
    let _ = RESOLVER.set(resolver.clone());
    Ok(RESOLVER.get().cloned().unwrap_or(resolver))
}

/// 取得（或初始化）計時專用、無快取的解析器。
fn timing_resolver() -> anyhow::Result<Arc<TokioResolver>> {
    if let Some(r) = TIMING_RESOLVER.get() {
        return Ok(r.clone());
    }
    let mut builder = Resolver::builder_tokio()?;
    // 關掉快取，每次都真的送出查詢，量到的才是 DNS 伺服器的實際反應時間
    builder.options_mut().cache_size = 0;
    let resolver = Arc::new(builder.build()?);
    let _ = TIMING_RESOLVER.set(resolver.clone());
    Ok(TIMING_RESOLVER.get().cloned().unwrap_or(resolver))
}

/// 讀取系統設定的 DNS 伺服器位址。
///
/// 家用環境常見的情況是這裡只會看到路由器的位址（192.168.x.1），
/// 因為路由器自己在做 DNS 轉發。
pub fn system_dns_servers() -> Vec<IpAddr> {
    match hickory_resolver::system_conf::read_system_conf() {
        Ok((config, _)) => {
            let mut seen = Vec::new();
            for ns in config.name_servers() {
                if !seen.contains(&ns.ip) {
                    seen.push(ns.ip);
                }
            }
            seen
        }
        Err(_) => Vec::new(),
    }
}

/// DNS 解析一次的結果。
#[derive(Debug, Clone, Copy)]
pub struct DnsProbe {
    /// 解析耗時；失敗時是 None
    pub elapsed_ms: Option<f64>,
    /// 是否真的拿到了位址
    pub resolved: bool,
}

/// 量測一次 DNS 解析耗時。
///
/// `seq` 用來輪替查詢的網域，呼叫端傳遞遞增的計數即可。
pub async fn measure(seq: u64) -> DnsProbe {
    let domain = PROBE_DOMAINS[(seq as usize) % PROBE_DOMAINS.len()];

    let resolver = match timing_resolver() {
        Ok(r) => r,
        Err(_) => {
            return DnsProbe {
                elapsed_ms: None,
                resolved: false,
            }
        }
    };

    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), resolver.lookup_ip(domain)).await;
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;

    match result {
        Ok(Ok(lookup)) => {
            let resolved = lookup.iter().next().is_some();
            DnsProbe {
                elapsed_ms: Some(elapsed),
                resolved,
            }
        }
        // 查詢失敗或逾時，兩者對使用者都是「DNS 有問題」
        Ok(Err(_)) | Err(_) => DnsProbe {
            elapsed_ms: None,
            resolved: false,
        },
    }
}

/// 反向查詢一個 IPv4 位址的主機名稱。
///
/// 用來認出路徑上的節點屬於哪一家電信商。查不到是常態（很多路由器沒設 PTR），
/// 所以失敗一律回 None，不當成錯誤。
pub async fn reverse(addr: Ipv4Addr) -> Option<String> {
    let resolver = system_resolver().ok()?;
    let arpa = to_arpa(addr);

    let lookup = tokio::time::timeout(Duration::from_secs(2), resolver.reverse_lookup(arpa))
        .await
        .ok()?
        .ok()?;

    lookup.answers().iter().find_map(|rec| match &rec.data {
        RData::PTR(ptr) => Some(ptr.to_string().trim_end_matches('.').to_string()),
        _ => None,
    })
}

/// 把 IPv4 位址轉成反查用的 `d.c.b.a.in-addr.arpa.` 名稱。
fn to_arpa(addr: Ipv4Addr) -> String {
    let o = addr.octets();
    format!("{}.{}.{}.{}.in-addr.arpa.", o[3], o[2], o[1], o[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arpa_name_is_reversed() {
        assert_eq!(
            to_arpa(Ipv4Addr::new(168, 95, 1, 1)),
            "1.1.95.168.in-addr.arpa."
        );
        assert_eq!(
            to_arpa(Ipv4Addr::new(8, 8, 8, 8)),
            "8.8.8.8.in-addr.arpa."
        );
    }

    #[test]
    fn probe_domains_rotate() {
        // seq 應該要循環走過每一個網域
        let n = PROBE_DOMAINS.len();
        let first = PROBE_DOMAINS[0];
        assert_eq!(PROBE_DOMAINS[n % n], first);
        assert_ne!(PROBE_DOMAINS[1 % n], first);
    }
}
