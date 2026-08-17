use std::fs::OpenOptions;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as implementation;

#[cfg(not(unix))]
mod fallback;
#[cfg(not(unix))]
use fallback as implementation;

pub(super) fn configure_private_ledger(options: &mut OpenOptions) {
    implementation::configure_private_ledger(options);
}
