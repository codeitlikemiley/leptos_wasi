//! Static asset path validation shared by the WASI HTTP transports.

use std::path::{Component, Path};

use thiserror::Error;

/// An error produced while validating a static asset path.
///
/// Static asset paths must remain relative after exactly one percent-decoding
/// pass. Callers should treat every error as an invalid client request and
/// must not invoke the configured filesystem or object-storage callback.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum StaticPathError {
    /// The path contains a percent sign that is not followed by two
    /// hexadecimal digits.
    #[error("static asset path contains malformed percent-encoding")]
    MalformedPercentEncoding,

    /// Percent-decoding produced bytes that are not valid UTF-8.
    #[error("static asset path is not valid UTF-8")]
    InvalidUtf8,

    /// A path separator was percent-encoded instead of appearing literally.
    #[error("static asset path contains an encoded separator")]
    EncodedSeparator,

    /// The path contains a backslash separator.
    #[error("static asset path contains a backslash separator")]
    BackslashSeparator,

    /// The path contains a NUL byte.
    #[error("static asset path contains a NUL byte")]
    NulByte,

    /// The path contains an ASCII control character.
    #[error("static asset path contains an ASCII control character")]
    ControlCharacter,

    /// The path is absolute or contains a platform root component.
    #[error("static asset path must be relative")]
    AbsolutePath,

    /// The path begins with a platform-specific prefix such as a Windows
    /// drive letter.
    #[error("static asset path contains a platform prefix")]
    PlatformPrefix,

    /// The path contains a current-directory component.
    #[error("static asset path contains a current-directory component")]
    CurrentDirectory,

    /// The path contains a parent-directory component.
    #[error("static asset path contains a parent-directory component")]
    ParentDirectory,

    /// The decoded path contains another percent-encoded byte sequence.
    ///
    /// Rejecting residual encodings prevents a downstream decoder from
    /// turning a validated name into a separator, control byte, or traversal
    /// component.
    #[error("static asset path contains residual percent-encoding")]
    ResidualPercentEncoding,
}

/// Maximum residual decoding passes performed while proving that a decoded
/// path cannot change meaning under further decoding.
///
/// Each pass is linear in the length of the probe, and an attacker-chosen
/// chain such as `%252525…2541` only shrinks by two bytes per pass. Bounding
/// the pass count keeps validation linear in the request path length instead
/// of quadratic. Legitimate names converge within one or two passes, so any
/// input that still decodes after this many passes is rejected as residual
/// encoding.
const MAX_RESIDUAL_DECODE_PASSES: usize = 8;

/// Decodes and normalizes a relative static asset path.
///
/// The input is the URI path remaining after the static route prefix and at
/// most one separating slash have been removed. An empty path is valid. The
/// returned value uses `/` between components and is safe to pass to a static
/// asset callback as a relative lookup key.
///
/// # Errors
///
/// Returns [`StaticPathError`] when the input is malformed, non-UTF-8,
/// absolute, contains unsafe path components, or could change meaning after
/// another percent-decoding pass.
pub(crate) fn normalize_static_path(
    raw_path: &str,
) -> Result<String, StaticPathError> {
    let decoded = percent_decode_once(raw_path)?;
    validate_decoded_bytes(&decoded)?;

    let decoded =
        String::from_utf8(decoded).map_err(|_| StaticPathError::InvalidUtf8)?;
    validate_residual_encoding(&decoded)?;
    validate_platform_prefix(&decoded)?;
    validate_components(&decoded)
}

fn percent_decode_once(raw_path: &str) -> Result<Vec<u8>, StaticPathError> {
    let bytes = raw_path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }

        let high = bytes
            .get(index + 1)
            .and_then(|byte| decode_hex_digit(*byte));
        let low = bytes
            .get(index + 2)
            .and_then(|byte| decode_hex_digit(*byte));
        let (Some(high), Some(low)) = (high, low) else {
            return Err(StaticPathError::MalformedPercentEncoding);
        };

        let byte = (high << 4) | low;
        if matches!(byte, b'/' | b'\\') {
            return Err(StaticPathError::EncodedSeparator);
        }

        decoded.push(byte);
        index += 3;
    }

    Ok(decoded)
}

const fn decode_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn validate_decoded_bytes(decoded: &[u8]) -> Result<(), StaticPathError> {
    for byte in decoded {
        match *byte {
            0 => return Err(StaticPathError::NulByte),
            b'\\' => return Err(StaticPathError::BackslashSeparator),
            1..=31 | 127 => {
                return Err(StaticPathError::ControlCharacter);
            }
            _ => {}
        }
    }

    Ok(())
}

fn validate_residual_encoding(decoded: &str) -> Result<(), StaticPathError> {
    let mut probe = decoded.to_owned();
    for _ in 0..MAX_RESIDUAL_DECODE_PASSES {
        let Some(next) = decode_residual_once(&probe)? else {
            return Ok(());
        };
        validate_platform_prefix(&next)
            .map_err(|_| StaticPathError::ResidualPercentEncoding)?;
        validate_components(&next)
            .map_err(|_| StaticPathError::ResidualPercentEncoding)?;
        probe = next;
    }

    Err(StaticPathError::ResidualPercentEncoding)
}

fn decode_residual_once(
    value: &str,
) -> Result<Option<String>, StaticPathError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut changed = false;
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (
                bytes
                    .get(index + 1)
                    .and_then(|byte| decode_hex_digit(*byte)),
                bytes
                    .get(index + 2)
                    .and_then(|byte| decode_hex_digit(*byte)),
            )
        {
            let byte = (high << 4) | low;
            if matches!(byte, 0..=31 | 127 | b'/' | b'\\') {
                return Err(StaticPathError::ResidualPercentEncoding);
            }
            decoded.push(byte);
            changed = true;
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    if !changed {
        return Ok(None);
    }
    let decoded = String::from_utf8(decoded)
        .map_err(|_| StaticPathError::ResidualPercentEncoding)?;
    Ok(Some(decoded))
}

fn validate_platform_prefix(decoded: &str) -> Result<(), StaticPathError> {
    let first_component = decoded.split('/').next().unwrap_or_default();
    let bytes = first_component.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(StaticPathError::PlatformPrefix);
    }

    Ok(())
}

fn validate_components(decoded: &str) -> Result<String, StaticPathError> {
    if decoded.starts_with('/') {
        return Err(StaticPathError::AbsolutePath);
    }

    for component in decoded.split('/') {
        match component {
            "." => return Err(StaticPathError::CurrentDirectory),
            ".." => return Err(StaticPathError::ParentDirectory),
            _ => {}
        }
    }

    let mut normalized = String::with_capacity(decoded.len());
    for component in Path::new(decoded).components() {
        let value = match component {
            Component::Normal(value) => value,
            Component::CurDir => {
                return Err(StaticPathError::CurrentDirectory);
            }
            Component::ParentDir => {
                return Err(StaticPathError::ParentDirectory);
            }
            Component::RootDir => {
                return Err(StaticPathError::AbsolutePath);
            }
            Component::Prefix(_) => {
                return Err(StaticPathError::PlatformPrefix);
            }
        };

        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized
            .push_str(value.to_str().ok_or(StaticPathError::InvalidUtf8)?);
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::{
        StaticPathError, normalize_static_path, validate_residual_encoding,
    };

    fn assert_rejected(path: &str, expected: StaticPathError) {
        let actual = normalize_static_path(path)
            .expect_err("the unsafe static path should be rejected");

        assert_eq!(actual, expected);
    }

    #[test]
    fn accepts_empty_path() {
        assert_eq!(normalize_static_path(""), Ok(String::new()));
    }

    #[test]
    fn accepts_nested_relative_path() {
        assert_eq!(
            normalize_static_path("pkg/images/logo.svg"),
            Ok("pkg/images/logo.svg".to_owned())
        );
    }

    #[test]
    fn accepts_filename_with_repeated_dots() {
        assert_eq!(
            normalize_static_path("pkg/app..min.js"),
            Ok("pkg/app..min.js".to_owned())
        );
    }

    #[test]
    fn decodes_valid_utf8_and_spaces() {
        assert_eq!(
            normalize_static_path("caf%C3%A9/my%20file.txt"),
            Ok("caf\u{e9}/my file.txt".to_owned())
        );
    }

    #[test]
    fn collapses_repeated_literal_separators() {
        assert_eq!(
            normalize_static_path("pkg//images///logo.svg"),
            Ok("pkg/images/logo.svg".to_owned())
        );
    }

    #[test]
    fn rejects_malformed_percent_escape() {
        assert_rejected("pkg/%", StaticPathError::MalformedPercentEncoding);
    }

    #[test]
    fn rejects_non_hex_percent_escape() {
        assert_rejected("pkg/%GG", StaticPathError::MalformedPercentEncoding);
    }

    #[test]
    fn rejects_invalid_percent_encoded_utf8() {
        assert_rejected("pkg/%FF", StaticPathError::InvalidUtf8);
    }

    #[test]
    fn rejects_percent_encoded_forward_slash() {
        assert_rejected("%2Fetc/passwd", StaticPathError::EncodedSeparator);
    }

    #[test]
    fn rejects_mixed_case_encoded_forward_slash() {
        assert_rejected("%2f..%2Fsecret", StaticPathError::EncodedSeparator);
    }

    #[test]
    fn rejects_percent_encoded_backslash() {
        assert_rejected("pkg/%5csecret", StaticPathError::EncodedSeparator);
    }

    #[test]
    fn rejects_literal_backslash() {
        assert_rejected("pkg\\secret", StaticPathError::BackslashSeparator);
    }

    #[test]
    fn rejects_percent_encoded_nul() {
        assert_rejected("pkg/%00", StaticPathError::NulByte);
    }

    #[test]
    fn rejects_literal_nul() {
        assert_rejected("pkg/\0", StaticPathError::NulByte);
    }

    #[test]
    fn rejects_percent_encoded_control_character() {
        assert_rejected("pkg/%1F", StaticPathError::ControlCharacter);
    }

    #[test]
    fn rejects_absolute_path() {
        assert_rejected("/etc/passwd", StaticPathError::AbsolutePath);
    }

    #[test]
    fn rejects_windows_drive_prefix_with_forward_slash() {
        assert_rejected("C:/secret", StaticPathError::PlatformPrefix);
    }

    #[test]
    fn rejects_windows_drive_relative_prefix() {
        assert_rejected("C:secret", StaticPathError::PlatformPrefix);
    }

    #[test]
    fn rejects_current_directory_component() {
        assert_rejected("pkg/./app.js", StaticPathError::CurrentDirectory);
    }

    #[test]
    fn rejects_parent_directory_component() {
        assert_rejected("pkg/../secret", StaticPathError::ParentDirectory);
    }

    #[test]
    fn rejects_percent_encoded_parent_directory_component() {
        assert_rejected("%2e%2e", StaticPathError::ParentDirectory);
    }

    #[test]
    fn rejects_double_encoded_forward_slash() {
        assert_rejected(
            "%252Fetc/passwd",
            StaticPathError::ResidualPercentEncoding,
        );
    }

    #[test]
    fn rejects_double_encoded_parent_directory() {
        assert_rejected(
            "%252e%252e/secret",
            StaticPathError::ResidualPercentEncoding,
        );
    }

    #[test]
    fn rejects_double_encoded_backslash() {
        assert_rejected(
            "pkg/%255csecret",
            StaticPathError::ResidualPercentEncoding,
        );
    }

    #[test]
    fn rejects_double_encoded_nul() {
        assert_rejected("pkg/%2500", StaticPathError::ResidualPercentEncoding);
    }

    #[test]
    fn rejects_repeatedly_encoded_separator() {
        assert_rejected(
            "pkg/%25252fsecret",
            StaticPathError::ResidualPercentEncoding,
        );
    }

    #[test]
    fn permits_safe_residual_percent_encoding() {
        assert_eq!(
            normalize_static_path("pkg/literal-%2541.txt"),
            Ok("pkg/literal-%41.txt".to_owned())
        );
    }

    /// Bytes that drive every branch of the validator: the escape character,
    /// both separators, the dot that forms traversal components, hex digits
    /// that complete an escape, a control byte, and one ordinary character.
    const INTERESTING: &[u8] = b"%/\\.aF0\x01";

    /// The properties every accepted path must satisfy, whatever the input.
    ///
    /// These are the guarantees the static callback relies on to treat its
    /// argument as a safe relative lookup key. Asserting them over generated
    /// input catches a class the example-based cases cannot: an unforeseen
    /// combination that slips through every individual rule.
    fn assert_accepted_path_is_safe(input: &str, accepted: &str) {
        let context = || format!("input {input:?} produced {accepted:?}");
        assert!(!accepted.starts_with('/'), "absolute: {}", context());
        assert!(!accepted.contains('\\'), "backslash: {}", context());
        assert!(!accepted.contains('\0'), "NUL: {}", context());
        assert!(
            !accepted.bytes().any(|byte| byte < 0x20 || byte == 0x7f),
            "control byte: {}",
            context()
        );
        // An empty result is valid: it means the request addressed the prefix
        // itself. Only a non-empty path is decomposed into components.
        if !accepted.is_empty() {
            for component in accepted.split('/') {
                assert!(component != ".", "current dir: {}", context());
                assert!(component != "..", "parent dir: {}", context());
                assert!(
                    !component.is_empty(),
                    "empty component: {}",
                    context()
                );
            }
        }
        // Whatever survives must also be stable: decoding the result again
        // must not turn it into something the rules above would reject.
        assert!(
            validate_residual_encoding(accepted).is_ok(),
            "residual encoding: {}",
            context()
        );
    }

    #[test]
    fn exhaustive_short_inputs_never_yield_an_unsafe_path() {
        // Every string up to four bytes over the interesting alphabet. The
        // alphabet is small enough that this is exhaustive rather than
        // sampled, so there is no seed and no flake.
        let mut checked = 0_u32;
        let mut accepted = 0_u32;
        for length in 0..=4 {
            let total = INTERESTING.len().pow(length as u32);
            for mut index in 0..total {
                let mut bytes = Vec::with_capacity(length);
                for _ in 0..length {
                    bytes.push(INTERESTING[index % INTERESTING.len()]);
                    index /= INTERESTING.len();
                }
                let Ok(input) = std::str::from_utf8(&bytes) else {
                    continue;
                };
                checked += 1;
                if let Ok(path) = normalize_static_path(input) {
                    accepted += 1;
                    assert_accepted_path_is_safe(input, &path);
                }
            }
        }
        assert!(checked > 4_000, "expected broad coverage, ran {checked}");
        assert!(
            accepted > 0,
            "every input was rejected; the test is vacuous"
        );
    }

    #[test]
    fn long_generated_inputs_stay_safe_and_bounded() {
        // A deterministic linear congruential sequence, so a failure is
        // reproducible and CI cannot flake on a lucky seed.
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 33) as usize
        };
        for _ in 0..2_000 {
            let length = next() % 64;
            let input: String = (0..length)
                .map(|_| INTERESTING[next() % INTERESTING.len()] as char)
                .collect();
            if let Ok(path) = normalize_static_path(&input) {
                assert_accepted_path_is_safe(&input, &path);
            }
        }
    }

    #[test]
    fn rejects_a_decoding_chain_longer_than_the_pass_budget() {
        let chain = format!("pkg/%{}41", "25".repeat(64));

        assert_rejected(&chain, StaticPathError::ResidualPercentEncoding);
    }

    #[test]
    fn accepts_a_decoding_chain_inside_the_pass_budget() {
        let chain = format!("pkg/%{}41", "25".repeat(2));

        assert_eq!(normalize_static_path(&chain), Ok("pkg/%2541".to_owned()));
    }

    #[test]
    fn long_decoding_chains_stay_linear_in_the_path_length() {
        // Before the pass budget, each pass shrank the probe by two bytes, so
        // this shape cost O(len^2). The budget keeps a hostile path from
        // costing more than a handful of linear scans.
        for length in [1_024usize, 16_384] {
            let chain = format!("pkg/%{}41", "25".repeat(length / 2));

            assert_rejected(&chain, StaticPathError::ResidualPercentEncoding);
        }
    }

    #[test]
    fn permits_residual_encoded_dot_inside_a_filename() {
        assert_eq!(
            normalize_static_path("pkg/app%252emin.js"),
            Ok("pkg/app%2emin.js".to_owned())
        );
    }
}
