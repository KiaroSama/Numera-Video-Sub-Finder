//! Fail-closed verification that the production build exposes no test-driver listener.
use super::Result;

#[cfg(not(windows))]
pub fn absent() -> Result<()> {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    for address in ["127.0.0.1:4445", "[::1]:4445"] {
        let address: SocketAddr = address.parse()?;
        match TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            Ok(_) => {
                return Err(format!(
                    "Production must expose no WebDriver listener; connection accepted on {address}"
                )
                .into());
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::AddrNotAvailable
                        | std::io::ErrorKind::NetworkUnreachable
                ) => {}
            Err(e) => {
                return Err(
                    format!("Listener absence could not be established on {address}: {e}").into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn table_absent(words: &[u32], bytes: usize, ipv6: bool) -> Result<()> {
    let (stride, state, port) = if ipv6 { (14, 12, 5) } else { (6, 0, 2) };
    if !(4..=1024 * 1024).contains(&bytes) || bytes > std::mem::size_of_val(words) {
        return Err("Native TCP listener table has an invalid byte length".into());
    }
    let count = *words
        .first()
        .ok_or("Native TCP listener table has no count")? as usize;
    let end = count
        .checked_mul(stride)
        .and_then(|length| length.checked_add(1))
        .ok_or("Native TCP listener table count overflow")?;
    if end > bytes / 4 {
        return Err("Native TCP listener table contains truncated rows".into());
    }
    for row in words[1..end].chunks_exact(stride) {
        if row[state] != 2 {
            return Err("Native TCP listener table returned a non-listening endpoint".into());
        }
        // The Win32 DWORD's low 16 bits contain the network-byte-order port.
        if u16::from_be(row[port] as u16) == 4445 {
            return Err(
                "Production must expose no WebDriver listener; native TCP table contains port4445"
                    .into(),
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn absent() -> Result<()> {
    use std::ffi::c_void;
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetExtendedTcpTable(
            table: *mut c_void,
            bytes: *mut u32,
            order: i32,
            family: u32,
            class: i32,
            reserved: u32,
        ) -> u32;
    }
    for (family, ipv6) in [(2, false), (23, true)] {
        let mut bytes = 0;
        // SAFETY: null table requests its size; size is a live DWORD, class3 is OWNER_PID_LISTENER.
        let code =
            unsafe { GetExtendedTcpTable(std::ptr::null_mut(), &mut bytes, 0, family, 3, 0) };
        if code != 122 {
            return Err(format!(
                "Native TCP listener table size query failed family={family} code={code}"
            )
            .into());
        }
        let mut verified = false;
        for _ in 0..3 {
            if !(4..=1024 * 1024).contains(&bytes) {
                return Err("Native TCP listener table exceeds its allocation budget".into());
            }
            // DWORD initialization/alignment avoids uninitialized bytes and unaligned row casts.
            let mut words = vec![0u32; (bytes as usize).div_ceil(4)];
            bytes = std::mem::size_of_val(words.as_slice()) as u32;
            // SAFETY: initialized DWORD-aligned storage has exactly the supplied byte capacity.
            let code = unsafe {
                GetExtendedTcpTable(words.as_mut_ptr().cast(), &mut bytes, 0, family, 3, 0)
            };
            if code == 122 {
                continue;
            }
            if code != 0 {
                return Err(format!(
                    "Native TCP listener table query failed family={family} code={code}"
                )
                .into());
            }
            table_absent(&words, bytes as usize, ipv6)?;
            verified = true;
            break;
        }
        if !verified {
            return Err("Native TCP listener table changed beyond three bounded snapshots".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_tables_reject_driver_ports_and_malformed_native_rows() {
        for ipv6 in [false, true] {
            let (stride, state, port) = if ipv6 { (14, 12, 5) } else { (6, 0, 2) };
            let mut words = vec![0u32; 1 + 2 * stride];
            words[0] = 2;
            words[1 + state] = 2;
            words[1 + port] = u16::to_be(443) as u32;
            words[1 + stride + state] = 2;
            words[1 + stride + port] = u16::to_be(4445) as u32;
            let bytes = words.len() * 4;
            assert!(
                table_absent(&words, bytes, ipv6)
                    .unwrap_err()
                    .to_string()
                    .contains("port4445")
            );
            words[1 + stride + port] = u16::to_be(4446) as u32;
            assert!(table_absent(&words, bytes, ipv6).is_ok());
            assert!(table_absent(&words, bytes - 1, ipv6).is_err());
            words[1 + stride + state] = 5;
            assert!(table_absent(&words, bytes, ipv6).is_err());
            words[0] = u32::MAX;
            assert!(table_absent(&words, bytes, ipv6).is_err());
        }
        assert!(table_absent(&[0], 4, false).is_ok());
        assert!(table_absent(&[], 0, false).is_err());
        assert!(table_absent(&[0], 8, false).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn hosted_native_tables_detect_real_ipv4_and_ipv6_listeners() {
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(
            std::env::var("RUNNER_ENVIRONMENT").as_deref(),
            Ok("github-hosted")
        );
        absent().unwrap();
        // One serial test owns this fixed driver port; GUI witnesses run after the Rust suites.
        for address in ["127.0.0.1:4445", "[::1]:4445"] {
            let listener = std::net::TcpListener::bind(address).unwrap();
            assert!(absent().unwrap_err().to_string().contains("port4445"));
            drop(listener);
            absent().unwrap();
        }
    }
}
