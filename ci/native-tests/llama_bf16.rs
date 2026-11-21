//! Exercises the actual linked ggml-cpu BF16 dot-product kernel against an independent
//! bit-widened FP64 product-sum oracle across aligned and unaligned SIMD buffers.

#![deny(unsafe_op_in_unsafe_fn)]

use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct GgmlBf16(u16);

const _: () = assert!(size_of::<GgmlBf16>() == size_of::<u16>());
const _: () = assert!(size_of::<f32>() == size_of::<u32>());

unsafe extern "C" {
    fn ggml_vec_dot_bf16(
        n: i32,
        s: *mut f32,
        bs: usize,
        x: *const GgmlBf16,
        bx: usize,
        y: *const GgmlBf16,
        by: usize,
        nrc: i32,
    );
    fn ggml_cpu_has_neon() -> i32;
    fn ggml_cpu_has_arm_fma() -> i32;
}

#[cfg(windows)]
fn runtime_arch() -> Result<String, &'static str> {
    #[repr(C)]
    struct SystemInfo {
        arch: u16,
        reserved: u16,
        page_size: u32,
        min_addr: *mut core::ffi::c_void,
        max_addr: *mut core::ffi::c_void,
        active_mask: usize,
        processors: u32,
        proc_type: u32,
        alloc_granularity: u32,
        proc_level: u16,
        proc_rev: u16,
    }
    unsafe extern "system" {
        fn GetNativeSystemInfo(info: *mut SystemInfo);
    }
    let mut info = core::mem::MaybeUninit::<SystemInfo>::zeroed();
    // SAFETY: GetNativeSystemInfo writes into a caller-owned SystemInfo buffer.
    let arch = unsafe {
        GetNativeSystemInfo(info.as_mut_ptr());
        info.assume_init().arch
    };
    Ok(match arch {
        12 => "aarch64".to_owned(),
        9 => "x86_64".to_owned(),
        _ => "unsupported".to_owned(),
    })
}

#[cfg(unix)]
fn runtime_arch() -> Result<String, &'static str> {
    #[repr(C)]
    struct UtsName {
        sysname: [u8; 65],
        nodename: [u8; 65],
        release: [u8; 65],
        version: [u8; 65],
        machine: [u8; 65],
        domainname: [u8; 65],
        pad: [u8; 128],
    }
    unsafe extern "C" {
        fn uname(buf: *mut UtsName) -> i32;
    }
    let mut info = core::mem::MaybeUninit::<UtsName>::zeroed();
    // SAFETY: uname populates the caller-provided utsname buffer on Linux.
    let rc = unsafe { uname(info.as_mut_ptr()) };
    if rc != 0 {
        return Err("Cannot inspect actual runtime architecture");
    }
    // SAFETY: uname returned 0, so machine is NUL-terminated ASCII.
    let raw = unsafe { info.assume_init().machine };
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let arch = std::str::from_utf8(&raw[..end]).map_err(|_| "Invalid uname machine bytes")?;
    Ok(if arch == "arm64" {
        "aarch64".to_owned()
    } else {
        arch.to_owned()
    })
}

fn widen(bits: u16) -> f32 {
    f32::from_bits(u32::from(bits) << 16)
}

fn fill(pattern: usize, i: usize) -> Result<(u16, u16), &'static str> {
    const ORDINARY: [u16; 8] = [
        0x3f80, 0xbf80, 0x4000, 0xc040, 0x3e80, 0xbe00, 0x4120, 0xc0c0,
    ];
    Ok(match pattern {
        0 => (0, ORDINARY[i % ORDINARY.len()]),
        1 => (
            ORDINARY[i % ORDINARY.len()],
            ORDINARY[(i * 3 + 1) % ORDINARY.len()],
        ),
        2 => (if i % 2 == 0 { 0x3f80 } else { 0xbf80 }, 0x3f80),
        3 => ((1 + (i % 127)) as u16, 0x3f80),
        4 => {
            let mut x = (((67 + (i % 121)) << 7) | (i % 128)) as u16;
            let y = (((187 - (i % 121)) << 7) | ((i * 7) % 128)) as u16;
            if i % 3 == 0 {
                x |= 0x8000;
            }
            (x, y)
        }
        5 => {
            let x = if i % 4 == 0 {
                0x3f81
            } else if i % 2 == 0 {
                0x3f80
            } else {
                0xbf80
            };
            (x, 0x3f80)
        }
        _ => return Err("Unknown numeric fixture"),
    })
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 || (args[1] != "--require-arm64" && args[1] != "--native") {
        return Err("Usage: nvsf-llama-bf16 --require-arm64|--native".into());
    }
    let arch = runtime_arch()?;
    let require_arm = args[1] == "--require-arm64";
    let compiled_arm = cfg!(all(target_arch = "aarch64", target_feature = "neon"));
    // SAFETY: ggml CPU feature queries take no pointers and inspect static CPU flags.
    let (neon, arm_fma) = unsafe { (ggml_cpu_has_neon(), ggml_cpu_has_arm_fma()) };
    if require_arm && (!compiled_arm || arch != "aarch64" || neon != 1) {
        return Err(
            "ARM64 witness needs actual native ARM64 and linked runtime NEON support".into(),
        );
    }
    if arch != "aarch64" && arch != "x86_64" {
        return Err("Unsupported actual runtime architecture".into());
    }

    let started = Instant::now();
    let lengths: [usize; 13] = [0, 1, 7, 8, 9, 15, 16, 17, 31, 32, 33, 257, 4608];
    let mut cases = 0_usize;
    let mut worst_fraction = 0.0_f64;
    let mut unaligned_cases = 0_usize;
    let denorm_min = f64::from(f32::from_bits(1));
    let u = f64::from(f32::EPSILON) / 2.0;

    for n in lengths {
        for offset in 0..4_usize {
            for pattern in 0..6_usize {
                if started.elapsed() > Duration::from_secs(5) {
                    return Err("BF16 witness exceeded 5-second internal deadline".into());
                }
                let count = n + offset + 8;
                let mut xs = vec![GgmlBf16(0x4a7b); count];
                let mut ys = vec![GgmlBf16(0xc91d); count];
                let mut expected = 0.0_f64;
                let mut absolute_sum = 0.0_f64;
                for i in 0..n {
                    let (a, b) = fill(pattern, i)?;
                    xs[offset + i] = GgmlBf16(a);
                    ys[offset + i] = GgmlBf16(b);
                    let product = f64::from(widen(a)) * f64::from(widen(b));
                    expected += product;
                    absolute_sum += product.abs();
                }
                let x_ptr = xs[offset..].as_ptr();
                let y_ptr = ys[offset..].as_ptr();
                if n >= 8
                    && (!(x_ptr as usize).is_multiple_of(16)
                        || !(y_ptr as usize).is_multiple_of(16))
                {
                    unaligned_cases += 1;
                }
                let x_before = xs.clone();
                let y_before = ys.clone();
                let mut output = [-123.0_f32, f32::NAN, 456.0_f32];
                // SAFETY: x_ptr and y_ptr have at least n + 8 initialized elements and output[1] is writable.
                unsafe {
                    ggml_vec_dot_bf16(n as i32, &raw mut output[1], 0, x_ptr, 0, y_ptr, 0, 1);
                }
                let steps = if compiled_arm && require_arm {
                    (n.div_ceil(8) + 16) as f64
                } else {
                    (n + 16) as f64
                };
                let gamma = (steps * u) / (1.0 - steps * u);
                let allowance = gamma * absolute_sum + (n as f64 + 1.0) * denorm_min;
                let error = (f64::from(output[1]) - expected).abs();
                if !output[1].is_finite()
                    || error > allowance
                    || output[0] != -123.0_f32
                    || output[2] != 456.0_f32
                    || xs != x_before
                    || ys != y_before
                {
                    eprintln!(
                        "BF16 mismatch n={n} offset={offset} pattern={pattern} actual={} expected={expected} bound={allowance}",
                        output[1]
                    );
                    return Err("Actual compiled BF16 kernel failed independent scalar oracle or buffer preservation".into());
                }
                if allowance > 0.0 {
                    worst_fraction = worst_fraction.max(error / allowance);
                }
                cases += 1;
            }
        }
    }
    if unaligned_cases == 0 {
        return Err("No actual unaligned SIMD storage was exercised".into());
    }
    println!(
        "{{\"schema\":1,\"passed\":true,\"tests\":{cases},\"failed\":0,\"skipped\":0,\"runtime_arch\":\"{arch}\",\"neon\":{neon},\"arm_fma\":{arm_fma},\"max_error_fraction\":{worst_fraction},\"oracle\":\"independent BF16 bit-widen/double-product-sum\"}}"
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("BF16 kernel witness failed: {error}");
        std::process::exit(1);
    }
}
