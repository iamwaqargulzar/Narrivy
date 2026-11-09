//! Peer identity and endpoint rules implementing ADR-0011 ("same-user local
//! IPC is a security boundary") for Windows named pipes.
//!
//! Everything here is portable and free of `unsafe`: a user is identified by
//! the binary security identifier (SID) of its access token, copied into a
//! fixed-size value so peer credentials stay `Copy` like their Unix
//! counterpart. The Win32 calls that read those SIDs live in `sys`.

#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};

use narrivy_ipc::error::IpcError;

/// Largest binary SID Windows defines (`SECURITY_MAX_SID_SIZE`): an 8-byte
/// header plus 15 sub-authorities of 4 bytes each.
pub const MAX_SID_BYTES: usize = 68;

const SID_HEADER_BYTES: usize = 8;
const SID_REVISION: u8 = 1;
const MAX_SUB_AUTHORITIES: usize = 15;

/// Binary security identifier of a Windows user, validated on construction.
///
/// Equality compares the full binary form, which is what `EqualSid` does.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserSid {
    bytes: [u8; MAX_SID_BYTES],
    len: u8,
}

impl UserSid {
    /// Validate and copy a binary SID (revision 1, sub-authority count
    /// within bounds, exact length). Anything else is rejected rather than
    /// truncated, so two different SIDs can never compare equal.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, IpcError> {
        if bytes.len() < SID_HEADER_BYTES || bytes.len() > MAX_SID_BYTES {
            return Err(IpcError::permission_denied("SID has an invalid length"));
        }
        if bytes[0] != SID_REVISION {
            return Err(IpcError::permission_denied("SID has an unknown revision"));
        }
        let count = usize::from(bytes[1]);
        if count > MAX_SUB_AUTHORITIES || bytes.len() != SID_HEADER_BYTES + 4 * count {
            return Err(IpcError::permission_denied(
                "SID length does not match its sub-authority count",
            ));
        }
        let mut copy = [0_u8; MAX_SID_BYTES];
        copy[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            bytes: copy,
            // Bounded by MAX_SID_BYTES (68) above.
            len: bytes.len() as u8,
        })
    }

    /// Parse the standard string form `S-1-<authority>-<sub>-...`.
    pub fn parse(text: &str) -> Result<Self, IpcError> {
        let invalid = || IpcError::bad_request("SID string is malformed");
        let mut parts = text.split('-');
        if parts.next() != Some("S") || parts.next() != Some("1") {
            return Err(invalid());
        }
        let authority_text = parts.next().ok_or_else(invalid)?;
        let authority = match authority_text
            .strip_prefix("0x")
            .or_else(|| authority_text.strip_prefix("0X"))
        {
            Some(hex) => u64::from_str_radix(hex, 16).map_err(|_| invalid())?,
            None => parse_decimal(authority_text).ok_or_else(invalid)?,
        };
        if authority >= 1 << 48 {
            return Err(invalid());
        }
        let mut bytes = [0_u8; MAX_SID_BYTES];
        bytes[0] = SID_REVISION;
        bytes[2..8].copy_from_slice(&authority.to_be_bytes()[2..8]);
        let mut count = 0;
        for part in parts {
            if count == MAX_SUB_AUTHORITIES {
                return Err(invalid());
            }
            let value = parse_decimal(part)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(invalid)?;
            let offset = SID_HEADER_BYTES + 4 * count;
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            count += 1;
        }
        // Bounded by MAX_SUB_AUTHORITIES (15) above.
        bytes[1] = count as u8;
        Self::from_bytes(&bytes[..SID_HEADER_BYTES + 4 * count])
    }

    /// Binary SID, exactly `GetLengthSid` bytes long.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    fn sub_authorities(&self) -> impl Iterator<Item = u32> + '_ {
        self.as_bytes()[SID_HEADER_BYTES..]
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
    }
}

/// Decimal digits only: no sign, no whitespace, no empty string.
fn parse_decimal(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The standard string form, as `ConvertSidToStringSidW` writes it: the
/// identifier authority is decimal below 2^32 and hexadecimal above.
impl fmt::Display for UserSid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut authority = [0_u8; 8];
        authority[2..8].copy_from_slice(&self.bytes[2..8]);
        let authority = u64::from_be_bytes(authority);
        write!(formatter, "S-{}-", self.bytes[0])?;
        if authority >> 32 == 0 {
            write!(formatter, "{authority}")?;
        } else {
            write!(formatter, "0x{authority:012X}")?;
        }
        for sub_authority in self.sub_authorities() {
            write!(formatter, "-{sub_authority}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for UserSid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "UserSid({self})")
    }
}

impl Serialize for UserSid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for UserSid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(|error| serde::de::Error::custom(error.message))
    }
}

/// OS-verified identity of the process at the other end of a named pipe.
///
/// `sid` is the user SID of the peer process's primary token, read through
/// the pipe's kernel-reported peer process id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerCredentials {
    pub sid: UserSid,
    /// Kernel-reported peer process id. Always present on Windows; kept
    /// optional so the shape matches the Unix transport's credentials.
    pub pid: Option<u32>,
}

impl PeerCredentials {
    pub fn is_same_user(&self, expected_sid: &UserSid) -> bool {
        self.sid == *expected_sid
    }

    /// Same-user authorization check (ADR-0011). Must pass before any
    /// privileged protocol handling.
    pub fn validate(&self, expected_sid: &UserSid) -> Result<(), IpcError> {
        if !self.is_same_user(expected_sid) {
            return Err(IpcError::permission_denied(format!(
                "peer SID {} does not match endpoint owner SID {expected_sid}",
                self.sid
            )));
        }
        Ok(())
    }
}

/// SDDL for every pipe instance: owner is the endpoint user and a protected
/// DACL grants full access to that user alone. Protection (`P`) blocks
/// inherited entries; no other account, group or SYSTEM is granted anything.
#[must_use]
pub fn endpoint_sddl(owner: &UserSid) -> String {
    format!("O:{owner}D:P(A;;GA;;;{owner})")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "S-1-5-21-3623811015-3361044348-30300820-1013";

    #[test]
    fn sid_string_round_trips_through_binary_form() -> Result<(), IpcError> {
        let sid = UserSid::parse(SAMPLE)?;
        assert_eq!(sid.to_string(), SAMPLE);
        assert_eq!(sid.as_bytes().len(), 8 + 4 * 5);
        assert_eq!(&sid.as_bytes()[..8], &[1, 5, 0, 0, 0, 0, 0, 5]);
        assert_eq!(UserSid::from_bytes(sid.as_bytes())?, sid);
        Ok(())
    }

    #[test]
    fn well_known_and_large_authority_sids_format_like_windows() -> Result<(), IpcError> {
        assert_eq!(UserSid::parse("S-1-5-18")?.to_string(), "S-1-5-18");
        assert_eq!(UserSid::parse("S-1-1-0")?.to_string(), "S-1-1-0");
        // Authorities of 2^32 and above use hexadecimal, as Windows does.
        let large = UserSid::parse("S-1-0x00FFFFFFFFFF-7")?;
        assert_eq!(large.to_string(), "S-1-0x00FFFFFFFFFF-7");
        Ok(())
    }

    #[test]
    fn malformed_sids_are_rejected() {
        for text in [
            "",
            "S-1",
            "S-2-5-18",
            "s-1-5-18",
            "S-1-5--18",
            "S-1-5-+18",
            "S-1-5-18 ",
            "S-1-5-4294967296",
            "S-1-281474976710656-1",
            "S-1-5-1-2-3-4-5-6-7-8-9-10-11-12-13-14-15-16",
        ] {
            assert!(UserSid::parse(text).is_err(), "{text:?} must be rejected");
        }
        assert!(UserSid::from_bytes(&[]).is_err());
        assert!(UserSid::from_bytes(&[2, 0, 0, 0, 0, 0, 0, 5]).is_err());
        // Declares one sub-authority but carries none.
        assert!(UserSid::from_bytes(&[1, 1, 0, 0, 0, 0, 0, 5]).is_err());
        assert!(UserSid::from_bytes(&[1, 16, 0, 0, 0, 0, 0, 5]).is_err());
        assert!(UserSid::from_bytes(&[0; MAX_SID_BYTES + 1]).is_err());
    }

    #[test]
    fn peer_validation_requires_the_exact_same_sid() -> Result<(), IpcError> {
        let owner = UserSid::parse(SAMPLE)?;
        let same = PeerCredentials {
            sid: UserSid::parse(SAMPLE)?,
            pid: Some(42),
        };
        assert!(same.is_same_user(&owner));
        same.validate(&owner)?;

        // Same prefix, different RID; and a strict prefix of the owner SID.
        for other in [
            "S-1-5-21-3623811015-3361044348-30300820-1014",
            "S-1-5-21-3623811015-3361044348-30300820",
            "S-1-5-18",
        ] {
            let peer = PeerCredentials {
                sid: UserSid::parse(other)?,
                pid: Some(42),
            };
            assert!(!peer.is_same_user(&owner));
            let error = peer.validate(&owner).err().map(|error| error.code);
            assert_eq!(
                error,
                Some(narrivy_ipc::error::IpcErrorCode::PermissionDenied)
            );
        }
        Ok(())
    }

    #[test]
    fn credentials_serialize_sid_as_string() -> Result<(), Box<dyn std::error::Error>> {
        let credentials = PeerCredentials {
            sid: UserSid::parse(SAMPLE)?,
            pid: None,
        };
        let json = serde_json::to_string(&credentials)?;
        assert_eq!(json, format!(r#"{{"sid":"{SAMPLE}","pid":null}}"#));
        assert_eq!(serde_json::from_str::<PeerCredentials>(&json)?, credentials);
        assert!(serde_json::from_str::<PeerCredentials>(r#"{"sid":"S-1","pid":null}"#).is_err());
        Ok(())
    }

    #[test]
    fn endpoint_sddl_grants_only_the_owner() -> Result<(), IpcError> {
        let owner = UserSid::parse(SAMPLE)?;
        assert_eq!(
            endpoint_sddl(&owner),
            format!("O:{SAMPLE}D:P(A;;GA;;;{SAMPLE})")
        );
        Ok(())
    }
}
