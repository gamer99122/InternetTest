//! Windows ICMP 探測。
//!
//! 刻意使用 iphlpapi 的 `IcmpSendEcho`，而不是 raw socket。raw socket 在 Windows
//! 上需要系統管理員權限，而這個工具的使用者是不懂電腦的人 —— 不能要求他們
//! 「右鍵、以系統管理員身分執行」。`IcmpSendEcho` 一般權限就能呼叫。
//!
//! 這個 API 是同步阻塞的，呼叫端要自己丟到 `spawn_blocking` 裡。

use std::ffi::c_void;
use std::mem;
use std::net::Ipv4Addr;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY, IP_OPTION_INFORMATION,
};

// iphlpapi 的 IP_STATUS 代碼，見 MSDN「IP_STATUS Codes」。
const IP_SUCCESS: u32 = 0;
const IP_DEST_NET_UNREACHABLE: u32 = 11002;
const IP_DEST_HOST_UNREACHABLE: u32 = 11003;
const IP_DEST_PROT_UNREACHABLE: u32 = 11004;
const IP_DEST_PORT_UNREACHABLE: u32 = 11005;
const IP_REQ_TIMED_OUT: u32 = 11010;
const IP_TTL_EXPIRED_TRANSIT: u32 = 11013;

/// 送出的酬載。32 bytes 跟 Windows 內建 ping.exe 一致，
/// 這樣量到的數字和使用者自己開 cmd 打 ping 會對得起來。
const PAYLOAD: &[u8; 32] = b"abcdefghijklmnopqrstuvwabcdefghi";

/// 一次 echo 的結果。
#[derive(Debug, Clone, Copy)]
pub struct Echo {
    /// 回應來源位址。TTL 逾時的情況下這是中途路由器的位址，traceroute 靠它。
    pub from: Option<Ipv4Addr>,
    /// 往返時間。只有 `Reached` 才有意義。
    pub rtt: Option<Duration>,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 目標有回應
    Reached,
    /// TTL 用完，回報的是中途某一跳（traceroute 用）
    TtlExpired,
    /// 逾時沒有回應
    TimedOut,
    /// 路由器明確回報不可達
    Unreachable,
    /// 其他錯誤（含 API 呼叫失敗）
    Failed,
}

/// 一個可重複使用的 ICMP handle。
///
/// `IcmpCreateFile` 拿到的 handle 不保證能並行呼叫，所以這裡不實作 `Sync`，
/// 每個探測工作各自持有一個。
pub struct IcmpHandle {
    handle: HANDLE,
}

// handle 可以在執行緒之間移動（spawn_blocking 需要），但不能被多執行緒同時使用。
unsafe impl Send for IcmpHandle {}

impl IcmpHandle {
    pub fn open() -> anyhow::Result<Self> {
        // SAFETY: 無參數呼叫，失敗時回傳 INVALID_HANDLE_VALUE。
        let handle = unsafe { IcmpCreateFile() };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            anyhow::bail!("無法建立 ICMP handle（IcmpCreateFile 失敗）");
        }
        Ok(IcmpHandle { handle })
    }

    /// 送一個 echo request 並等待回應。
    ///
    /// `ttl` 為 None 時使用系統預設值（一般是 128）；指定數值是給 traceroute 用的。
    ///
    /// 回傳的 `rtt` 是用 `Instant` 量的實際耗時，而不是 API 回報的
    /// `RoundTripTime` —— 後者只有整數毫秒的解析度，在區域網路內幾乎永遠是 0，
    /// 那樣就完全算不出抖動了。
    pub fn echo(&self, dest: Ipv4Addr, timeout: Duration, ttl: Option<u8>) -> Echo {
        // 回覆緩衝區至少要 sizeof(ICMP_ECHO_REPLY) + 酬載大小 + 8，見 MSDN。
        // 多給一些空間讓 API 有機會塞進錯誤訊息。
        let mut reply_buf = vec![0u8; mem::size_of::<ICMP_ECHO_REPLY>() + PAYLOAD.len() + 64];

        let opts = ttl.map(|t| IP_OPTION_INFORMATION {
            Ttl: t,
            Tos: 0,
            Flags: 0,
            OptionsSize: 0,
            OptionsData: ptr::null_mut(),
        });
        let opts_ptr = opts
            .as_ref()
            .map(|o| o as *const IP_OPTION_INFORMATION)
            .unwrap_or(ptr::null());

        // IcmpSendEcho 的位址參數是 network byte order 的 u32。
        let dest_be = u32::from_ne_bytes(dest.octets());

        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;

        let started = Instant::now();
        // SAFETY: handle 有效；requestdata 指向存活的 PAYLOAD；replybuffer 是我們
        // 剛配置且長度正確的 Vec；opts 若存在則在本次呼叫期間都還活著。
        let replies = unsafe {
            IcmpSendEcho(
                self.handle,
                dest_be,
                PAYLOAD.as_ptr() as *const c_void,
                PAYLOAD.len() as u16,
                opts_ptr,
                reply_buf.as_mut_ptr() as *mut c_void,
                reply_buf.len() as u32,
                timeout_ms,
            )
        };
        let elapsed = started.elapsed();

        if replies == 0 {
            // 沒有任何回覆。可能是逾時，也可能是位址無效之類的錯誤，
            // 對使用者來說都是「不通」，不需要細分。
            return Echo {
                from: None,
                rtt: None,
                outcome: Outcome::TimedOut,
            };
        }

        // SAFETY: replies > 0 代表 API 已經在緩衝區開頭填好一個 ICMP_ECHO_REPLY。
        // 用 read_unaligned 避免對 Vec<u8> 的位址做對齊假設。
        let reply: ICMP_ECHO_REPLY =
            unsafe { ptr::read_unaligned(reply_buf.as_ptr() as *const ICMP_ECHO_REPLY) };

        let from = Ipv4Addr::from(reply.Address.to_ne_bytes());

        match reply.Status {
            IP_SUCCESS => Echo {
                from: Some(from),
                rtt: Some(elapsed),
                outcome: Outcome::Reached,
            },
            IP_TTL_EXPIRED_TRANSIT => Echo {
                from: Some(from),
                rtt: Some(elapsed),
                outcome: Outcome::TtlExpired,
            },
            IP_DEST_NET_UNREACHABLE
            | IP_DEST_HOST_UNREACHABLE
            | IP_DEST_PROT_UNREACHABLE
            | IP_DEST_PORT_UNREACHABLE => Echo {
                from: Some(from),
                rtt: None,
                outcome: Outcome::Unreachable,
            },
            IP_REQ_TIMED_OUT => Echo {
                from: None,
                rtt: None,
                outcome: Outcome::TimedOut,
            },
            _ => Echo {
                from: None,
                rtt: None,
                outcome: Outcome::Failed,
            },
        }
    }
}

impl Drop for IcmpHandle {
    fn drop(&mut self) {
        // SAFETY: handle 由 IcmpCreateFile 產生且只在這裡關閉一次。
        unsafe {
            IcmpCloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_open_handle_without_admin() {
        // 這個測試的重點就是「不需要系統管理員權限」。
        // 如果哪天改成 raw socket，這裡會第一個爆掉。
        let h = IcmpHandle::open();
        assert!(h.is_ok(), "IcmpCreateFile 失敗：{:?}", h.err());
    }

    #[test]
    fn loopback_responds() {
        let h = IcmpHandle::open().expect("開啟 ICMP handle");
        let r = h.echo(Ipv4Addr::LOCALHOST, Duration::from_millis(1000), None);
        assert_eq!(r.outcome, Outcome::Reached, "127.0.0.1 應該要通");
        assert!(r.rtt.is_some());
    }

    #[test]
    fn unroutable_address_times_out() {
        let h = IcmpHandle::open().expect("開啟 ICMP handle");
        // TEST-NET-1（RFC 5737），保證不會有人回應
        let r = h.echo(
            Ipv4Addr::new(192, 0, 2, 1),
            Duration::from_millis(300),
            None,
        );
        assert_ne!(r.outcome, Outcome::Reached);
        assert!(r.rtt.is_none() || r.outcome != Outcome::Reached);
    }

    #[test]
    fn ttl_one_reports_a_hop_not_the_destination() {
        let h = IcmpHandle::open().expect("開啟 ICMP handle");
        // TTL=1 打外網，第一跳（家裡的路由器）應該回 TTL expired。
        // 沒有網路的機器上會逾時，所以兩種結果都接受，只是不能是 Reached。
        let r = h.echo(
            Ipv4Addr::new(1, 1, 1, 1),
            Duration::from_millis(1000),
            Some(1),
        );
        assert_ne!(
            r.outcome,
            Outcome::Reached,
            "TTL=1 不可能直接到達 1.1.1.1"
        );
    }
}
