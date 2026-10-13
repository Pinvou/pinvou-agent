use crate::{
    Artifact, RequestKind, RequestPlan, ResponseHeaders, StrongEtag, TransferError, decimal,
    split_pair,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartReason {
    PreconditionFailed,
    RangeUnsatisfiable,
}

#[derive(Clone, Copy)]
pub enum ResponseDecision<'a> {
    Transfer(BodyPlan<'a>),
    Restart(RestartReason),
    VerifyExisting(Artifact<'a>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    ReplaceFromZero,
    Append { offset: u64 },
}

#[derive(Clone, Copy)]
pub struct BodyPlan<'a> {
    target: Artifact<'a>,
    mode: WriteMode,
    response_length: u64,
}
impl<'a> BodyPlan<'a> {
    pub fn target(self) -> Artifact<'a> {
        self.target
    }
    pub fn write_mode(self) -> WriteMode {
        self.mode
    }
    pub fn expected_response_length(self) -> u64 {
        self.response_length
    }
    pub fn receive(self) -> BodyProgress<'a> {
        BodyProgress {
            plan: self,
            state: ReceiveState::Receiving(0),
        }
    }
}

pub fn plan_response<'a>(
    request: RequestPlan<'a>,
    status: u16,
    headers: ResponseHeaders<'a>,
) -> Result<ResponseDecision<'a>, TransferError> {
    if request.kind() == RequestKind::VerifyExisting {
        return if status == 416 {
            Ok(ResponseDecision::VerifyExisting(request.target()))
        } else {
            Err(TransferError::LocalVerificationRequired)
        };
    }
    if status == 412 {
        return Ok(ResponseDecision::Restart(RestartReason::PreconditionFailed));
    }
    if status == 416 {
        return Ok(ResponseDecision::Restart(RestartReason::RangeUnsatisfiable));
    }
    if let Some(encoding) = headers.content_encoding
        && !encoding.eq_ignore_ascii_case(b"identity")
    {
        return Err(TransferError::UnsupportedEncoding);
    }
    let target = request.target();
    let (mode, response_length) = match status {
        200 => {
            if headers.content_range.is_some() {
                return Err(TransferError::UnexpectedPartialResponse);
            }
            (WriteMode::ReplaceFromZero, target.size())
        }
        206 => {
            let RequestKind::Range { offset, validator } = request.kind() else {
                return Err(TransferError::UnexpectedPartialResponse);
            };
            let etag = headers.etag.ok_or(TransferError::ValidatorMismatch)?;
            if StrongEtag::parse(etag)? != validator {
                return Err(TransferError::ValidatorMismatch);
            }
            let (start, end, total) = content_range(
                headers
                    .content_range
                    .ok_or(TransferError::InvalidContentRange)?,
            )?;
            if start != offset || total != target.size() {
                return Err(TransferError::RangeMismatch);
            }
            (WriteMode::Append { offset }, end - start + 1)
        }
        _ => return Err(TransferError::HttpStatusRefused(status)),
    };
    if let Some(length) = headers.content_length
        && decimal(length) != Some(response_length)
    {
        return Err(TransferError::InvalidContentLength);
    }
    Ok(ResponseDecision::Transfer(BodyPlan {
        target,
        mode,
        response_length,
    }))
}

fn content_range(bytes: &[u8]) -> Result<(u64, u64, u64), TransferError> {
    let invalid = TransferError::InvalidContentRange;
    let (unit, range) = split_pair(bytes, b' ').ok_or(invalid)?;
    if !unit.eq_ignore_ascii_case(b"bytes") {
        return Err(invalid);
    }
    let (span, total) = split_pair(range, b'/').ok_or(invalid)?;
    let (start, end) = split_pair(span, b'-').ok_or(invalid)?;
    let (start, end, total) = (
        decimal(start).ok_or(invalid)?,
        decimal(end).ok_or(invalid)?,
        decimal(total).ok_or(invalid)?,
    );
    if end < start || end >= total {
        return Err(invalid);
    }
    Ok((start, end, total))
}

enum ReceiveState {
    Receiving(u64),
    Failed(TransferError),
    Finished,
}

/// Counts decoded transfer-framing body bytes; no file is written or verified.
pub struct BodyProgress<'a> {
    plan: BodyPlan<'a>,
    state: ReceiveState,
}
impl<'a> BodyProgress<'a> {
    pub fn observe_bytes(&mut self, actual_count: u64) -> Result<(), TransferError> {
        let received = match self.state {
            ReceiveState::Receiving(received) => received,
            ReceiveState::Failed(error) => return Err(error),
            ReceiveState::Finished => return Err(TransferError::BodyClosed),
        };
        match received
            .checked_add(actual_count)
            .filter(|count| *count <= self.plan.response_length)
        {
            Some(received) => {
                self.state = ReceiveState::Receiving(received);
                Ok(())
            }
            None => {
                self.state = ReceiveState::Failed(TransferError::BodyTooLong);
                Err(TransferError::BodyTooLong)
            }
        }
    }
    pub fn abort(&mut self) -> Result<(), TransferError> {
        match self.state {
            ReceiveState::Receiving(_) => {
                self.state = ReceiveState::Failed(TransferError::BodyAborted);
                Err(TransferError::BodyAborted)
            }
            ReceiveState::Failed(error) => Err(error),
            ReceiveState::Finished => Err(TransferError::BodyClosed),
        }
    }
    pub fn finish(&mut self) -> Result<SegmentReceipt<'a>, TransferError> {
        match self.state {
            ReceiveState::Receiving(received) if received == self.plan.response_length => {
                self.state = ReceiveState::Finished;
                let start = match self.plan.mode {
                    WriteMode::ReplaceFromZero => 0,
                    WriteMode::Append { offset } => offset,
                };
                Ok(SegmentReceipt {
                    target: self.plan.target,
                    next_offset: start + received,
                })
            }
            ReceiveState::Receiving(_) => {
                self.state = ReceiveState::Failed(TransferError::BodyTruncated);
                Err(TransferError::BodyTruncated)
            }
            ReceiveState::Failed(error) => Err(error),
            ReceiveState::Finished => Err(TransferError::BodyClosed),
        }
    }
}

/// Only an exact segment byte count. It is neither hash/signature verification
/// nor file ownership, platform authenticity or installation permission.
#[derive(Clone, Copy)]
pub struct SegmentReceipt<'a> {
    target: Artifact<'a>,
    next_offset: u64,
}
impl<'a> SegmentReceipt<'a> {
    pub fn target(self) -> Artifact<'a> {
        self.target
    }
    pub fn next_offset(self) -> u64 {
        self.next_offset
    }
    pub fn has_full_byte_count(self) -> bool {
        self.next_offset == self.target.size()
    }
}
