use std::fs::{File, OpenOptions, Permissions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

pub(super) fn configure_private_ledger(options: &mut OpenOptions) {
    options.mode(0o600);
}

pub(super) fn harden_private_runtime_dir(path: &Path) -> io::Result<()> {
    std::fs::set_permissions(path, Permissions::from_mode(0o700))
}

pub(super) fn harden_private_ledger(file: &File) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(0o600))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn runtime_directory_and_ledger_are_owner_only() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "pinvou-os-private-ledger-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        harden_private_runtime_dir(&root).unwrap();

        let ledger = root.join("events.v1.jsonl");
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        configure_private_ledger(&mut options);
        let file = options.open(&ledger).unwrap();
        harden_private_ledger(&file).unwrap();

        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&ledger).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(file);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
