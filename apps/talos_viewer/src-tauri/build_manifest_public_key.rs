use std::{fs, path::Path};

pub fn persist_validated_pkcs1_rsa_public_key(source: &Path, target: &Path) -> Result<(), String> {
    let bytes = fs::read(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    validate_pkcs1_rsa_public_key_der(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", source.display()))?;
    fs::write(target, &bytes)
        .map_err(|error| format!("failed to write {}: {error}", target.display()))?;
    let persisted = fs::read(target)
        .map_err(|error| format!("failed to re-read {}: {error}", target.display()))?;
    if persisted != bytes {
        return Err(format!(
            "persisted key at {} differs from the configured key",
            target.display()
        ));
    }
    Ok(())
}

fn validate_pkcs1_rsa_public_key_der(bytes: &[u8]) -> Result<(), String> {
    let mut outer_cursor = 0;
    let sequence = read_der_value(bytes, &mut outer_cursor, 0x30, "RSA public-key sequence")?;
    if outer_cursor != bytes.len() {
        return Err("trailing bytes follow the RSA public-key sequence".to_string());
    }

    let mut sequence_cursor = 0;
    let modulus = read_der_value(sequence, &mut sequence_cursor, 0x02, "RSA modulus")?;
    let exponent = read_der_value(sequence, &mut sequence_cursor, 0x02, "RSA exponent")?;
    if sequence_cursor != sequence.len() {
        return Err("RSA public-key sequence has unexpected fields".to_string());
    }

    let modulus = positive_der_integer(modulus, "RSA modulus")?;
    let modulus_bits = modulus
        .len()
        .checked_mul(8)
        .and_then(|bits| bits.checked_sub(modulus[0].leading_zeros() as usize))
        .ok_or_else(|| "RSA modulus size overflow".to_string())?;
    if !(2048..=8192).contains(&modulus_bits) {
        return Err(format!(
            "RSA modulus must contain 2048 through 8192 bits, found {modulus_bits}"
        ));
    }

    let exponent = positive_der_integer(exponent, "RSA exponent")?;
    if exponent.len() > std::mem::size_of::<u64>() {
        return Err("RSA exponent is too large".to_string());
    }
    let exponent = exponent
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
    if exponent < 3 || exponent % 2 == 0 {
        return Err("RSA exponent must be an odd integer of at least 3".to_string());
    }
    Ok(())
}

fn positive_der_integer<'a>(value: &'a [u8], label: &str) -> Result<&'a [u8], String> {
    let first = *value.first().ok_or_else(|| format!("{label} is empty"))?;
    if first & 0x80 != 0 {
        return Err(format!("{label} is negative"));
    }
    if value.len() > 1 && first == 0 {
        if value[1] & 0x80 == 0 {
            return Err(format!("{label} has redundant leading zero bytes"));
        }
        return Ok(&value[1..]);
    }
    if first == 0 {
        return Err(format!("{label} must be positive"));
    }
    Ok(value)
}

fn read_der_value<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    expected_tag: u8,
    label: &str,
) -> Result<&'a [u8], String> {
    let tag = take_byte(bytes, cursor, label)?;
    if tag != expected_tag {
        return Err(format!(
            "{label} has DER tag 0x{tag:02x}, expected 0x{expected_tag:02x}"
        ));
    }
    let first_length = take_byte(bytes, cursor, label)?;
    let length = if first_length & 0x80 == 0 {
        usize::from(first_length)
    } else {
        let length_octets = usize::from(first_length & 0x7f);
        if length_octets == 0 {
            return Err(format!("{label} uses an indefinite DER length"));
        }
        if length_octets > std::mem::size_of::<usize>() {
            return Err(format!("{label} DER length is too large"));
        }
        let first_octet = *bytes
            .get(*cursor)
            .ok_or_else(|| format!("{label} DER length is truncated"))?;
        if first_octet == 0 {
            return Err(format!("{label} DER length is not minimally encoded"));
        }
        let mut length = 0_usize;
        for _ in 0..length_octets {
            let octet = take_byte(bytes, cursor, label)?;
            length = length
                .checked_mul(256)
                .and_then(|value| value.checked_add(usize::from(octet)))
                .ok_or_else(|| format!("{label} DER length overflow"))?;
        }
        if length < 128 {
            return Err(format!("{label} DER length is not minimally encoded"));
        }
        length
    };
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| format!("{label} DER length overflow"))?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| format!("{label} is truncated"))?;
    *cursor = end;
    Ok(value)
}

fn take_byte(bytes: &[u8], cursor: &mut usize, label: &str) -> Result<u8, String> {
    let byte = *bytes
        .get(*cursor)
        .ok_or_else(|| format!("{label} is truncated"))?;
    *cursor += 1;
    Ok(byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn synthetic_2048_bit_key() -> Vec<u8> {
        let mut key = vec![0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00, 0x80];
        key.extend(std::iter::repeat_n(0, 255));
        key.extend([0x02, 0x03, 0x01, 0x00, 0x01]);
        key
    }

    #[test]
    fn accepts_canonical_pkcs1_rsa_public_key() {
        validate_pkcs1_rsa_public_key_der(&synthetic_2048_bit_key())
            .expect("canonical 2048-bit key");
    }

    #[test]
    fn rejects_empty_truncated_and_trailing_key_data() {
        assert!(validate_pkcs1_rsa_public_key_der(&[]).is_err());

        let mut truncated = synthetic_2048_bit_key();
        truncated.pop();
        assert!(validate_pkcs1_rsa_public_key_der(&truncated).is_err());

        let mut trailing = synthetic_2048_bit_key();
        trailing.push(0);
        assert!(validate_pkcs1_rsa_public_key_der(&trailing).is_err());
    }

    #[test]
    fn rejects_undersized_modulus_and_even_exponent() {
        let mut undersized = synthetic_2048_bit_key();
        undersized[9] = 0x40;
        assert!(validate_pkcs1_rsa_public_key_der(&undersized).is_err());

        let mut even_exponent = synthetic_2048_bit_key();
        let last = even_exponent.last_mut().expect("exponent byte");
        *last = 0x02;
        assert!(validate_pkcs1_rsa_public_key_der(&even_exponent).is_err());
    }

    #[test]
    fn persistence_fails_closed_for_missing_and_malformed_sources() {
        let root = std::env::temp_dir().join(format!(
            "talos-viewer-manifest-key-test-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("test directory");
        let missing = root.join("missing.der");
        let malformed = root.join("malformed.der");
        let target = root.join("embedded.der");

        assert!(persist_validated_pkcs1_rsa_public_key(&missing, &target).is_err());
        fs::write(&malformed, [0x30, 0x00]).expect("malformed fixture");
        assert!(persist_validated_pkcs1_rsa_public_key(&malformed, &target).is_err());
        assert!(!target.exists());

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn persistence_round_trips_a_valid_key() {
        let root = std::env::temp_dir().join(format!(
            "talos-viewer-manifest-key-test-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("test directory");
        let source = root.join("source.der");
        let target = root.join("embedded.der");
        let key = synthetic_2048_bit_key();
        fs::write(&source, &key).expect("valid fixture");

        persist_validated_pkcs1_rsa_public_key(&source, &target).expect("persist valid key");
        assert_eq!(fs::read(&target).expect("embedded key"), key);

        fs::remove_dir_all(root).expect("remove test directory");
    }
}
