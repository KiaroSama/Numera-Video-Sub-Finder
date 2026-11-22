use super::{Result, layout, unsigned_pe, valid_pin};

fn fixture(signed: bool) -> Vec<u8> {
    let mut bytes = vec![0; if signed { 528 } else { 512 }];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&128_u32.to_le_bytes());
    bytes[128..132].copy_from_slice(b"PE\0\0");
    for (offset, value) in [(132, 0x8664_u16), (134, 1), (148, 240), (152, 0x20b)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[212..216].copy_from_slice(&512_u32.to_le_bytes());
    bytes[260..264].copy_from_slice(&16_u32.to_le_bytes());
    if signed {
        for (offset, value) in [(216, 123_u32), (296, 512), (300, 16), (512, 10)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[516..518].copy_from_slice(&0x200_u16.to_le_bytes());
        bytes[518..520].copy_from_slice(&2_u16.to_le_bytes());
        bytes[520] = 0x30;
    }
    bytes
}

pub(super) fn structural() -> Result<()> {
    let expected = unsigned_pe(fixture(false))?;
    if unsigned_pe(fixture(true))? != expected {
        return Err("Exact signature-only normalization differs.".into());
    }
    for offset in [0, 60, 128, 152, 260, 296, 300, 512, 516, 518, 527] {
        let mut bad = fixture(true);
        bad[offset] ^= 0xff;
        if unsigned_pe(bad).is_ok() {
            return Err(format!(
                "Expected Authenticode rejection did not occur at offset {offset}."
            )
            .into());
        }
    }
    let mut bad = fixture(true);
    bad[450] = 0x7f;
    if unsigned_pe(bad)? == expected
        || valid_pin("invalid")
        || layout(&fixture(false))?.certificate.is_some()
    {
        return Err(
            "Normalization hid a mutation or admitted an invalid pin or unsigned image.".into(),
        );
    }
    #[cfg(windows)]
    {
        let root = crate::ci_guard::state_dir(&crate::ci_guard::root())?;
        let temp = tempfile::Builder::new()
            .prefix("authenticode-structural-tests-")
            .tempdir_in(root)?;
        let plain = temp.path().join("plain 日本.exe");
        let signed = temp.path().join("signed 日本.exe");
        std::fs::write(&plain, fixture(false))?;
        std::fs::write(&signed, fixture(true))?;
        if super::unsigned_file(&plain)? != super::unsigned_file(&signed)?
            || super::verify(&plain, "invalid").is_ok()
            || super::verify(&plain, &"0".repeat(64)).is_ok()
        {
            return Err("Authenticode file normalization or invalid-input contract failed.".into());
        }
        temp.close()?;
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn normalizes_only_signature_fields() {
    // The file-level Windows adapter is separately exercised by ci-authenticode-tests.
    assert_eq!(
        unsigned_pe(fixture(false)).expect("plain structure"),
        unsigned_pe(fixture(true)).expect("signed structure")
    );
}
