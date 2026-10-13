use crate::{MAX_SAFE_SIZE, StrongEtag, TransferError};

/// Structural expected identity. The owner independently verifies signed target
/// binding and current qualification; this constructor authenticates neither.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Artifact<'a> {
    package_id: &'a str,
    size: u64,
    sha256: [u8; 32],
}
impl<'a> Artifact<'a> {
    pub fn new(package_id: &'a str, size: u64, sha256: &str) -> Result<Self, TransferError> {
        let identifier = package_id.as_bytes();
        if identifier.is_empty()
            || identifier.len() > 128
            || !identifier[0].is_ascii_alphanumeric()
            || !identifier
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(byte))
            || size == 0
            || size > MAX_SAFE_SIZE
            || sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TransferError::InvalidArtifact);
        }
        let mut digest = [0; 32];
        for (output, pair) in digest.iter_mut().zip(sha256.as_bytes().chunks_exact(2)) {
            let nibble = |byte: u8| {
                if byte <= b'9' {
                    byte - b'0'
                } else {
                    byte - b'a' + 10
                }
            };
            *output = nibble(pair[0]) * 16 + nibble(pair[1]);
        }
        Ok(Self {
            package_id,
            size,
            sha256: digest,
        })
    }
    pub fn package_id(self) -> &'a str {
        self.package_id
    }
    pub fn size(self) -> u64 {
        self.size
    }
    pub fn sha256(self) -> [u8; 32] {
        self.sha256
    }
}

/// External protected-owner/provider evidence, never inferred from equal ETags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceContinuity {
    #[default]
    Unproven,
    SameImmutableEntity,
}

/// The owner proves the actual retained bytes/length under exclusive cache
/// ownership. Parsed fields alone do not establish that proof or persistence.
#[derive(Clone, Copy)]
pub struct PrefixState<'a> {
    bound_artifact: Option<Artifact<'a>>,
    length: u64,
    validator: Option<StrongEtag<'a>>,
}
impl<'a> PrefixState<'a> {
    pub fn new(
        bound_artifact: Option<Artifact<'a>>,
        length: u64,
        validator: Option<StrongEtag<'a>>,
    ) -> Result<Self, TransferError> {
        if length > MAX_SAFE_SIZE {
            return Err(TransferError::InvalidPrefixLength);
        }
        Ok(Self {
            bound_artifact,
            length,
            validator,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RequestKind<'a> {
    Full,
    Range {
        offset: u64,
        validator: StrongEtag<'a>,
    },
    VerifyExisting,
}

/// Immutable binding used by response planning; there is no second target
/// argument with which a caller can combine an old response and a new artifact.
#[derive(Clone, Copy)]
pub struct RequestPlan<'a> {
    target: Artifact<'a>,
    kind: RequestKind<'a>,
}
impl<'a> RequestPlan<'a> {
    pub fn target(self) -> Artifact<'a> {
        self.target
    }
    pub fn kind(self) -> RequestKind<'a> {
        self.kind
    }
}

pub fn plan_request<'a>(
    target: Artifact<'a>,
    prefix: Option<PrefixState<'a>>,
    continuity: SourceContinuity,
) -> RequestPlan<'a> {
    let kind = match prefix {
        Some(prefix) if prefix.length == target.size => RequestKind::VerifyExisting,
        Some(prefix)
            if prefix.length > 0
                && prefix.length < target.size
                && prefix.bound_artifact == Some(target)
                && continuity == SourceContinuity::SameImmutableEntity =>
        {
            match prefix.validator {
                Some(validator) => RequestKind::Range {
                    offset: prefix.length,
                    validator,
                },
                None => RequestKind::Full,
            }
        }
        _ => RequestKind::Full,
    };
    RequestPlan { target, kind }
}
