use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt as _;

pub(super) fn configure_private_ledger(options: &mut OpenOptions) {
    options.mode(0o600);
}
