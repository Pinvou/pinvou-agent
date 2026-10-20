use crate::*;
#[test]
fn actual_close_reopen_checks_original_identity_and_requires_writer_retirement() {
    let path = std::env::temp_dir().join(format!(
        "pinvou-native-reopen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    {
        let directory = pinvou_upgrade_custody::directory(&path).unwrap();
        let (name, writer) = fresh(&directory).unwrap();
        let original = pinvou_upgrade_custody::identity(&writer).unwrap();
        assert!(reopen(&directory, &name, original).is_err());
        drop(writer);
        let original_read = reopen(&directory, &name, original).unwrap();
        assert!(pinvou_upgrade_custody::identity(&original_read).unwrap() == original);
        drop(original_read);
        std::fs::rename(path.join(&name), path.join("original")).unwrap();
        std::fs::write(path.join(&name), b"replacement").unwrap();
        assert!(matches!(
            reopen(&directory, &name, original),
            Err(Error::UnsafeObject)
        ));
        std::fs::remove_file(path.join(&name)).unwrap();
        let junction_target = path.join("target");
        std::fs::create_dir(&junction_target).unwrap();
        let linked = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(path.join(&name))
            .arg(&junction_target)
            .output()
            .unwrap();
        assert!(
            linked.status.success(),
            "actual local junction fixture must be available"
        );
        assert!(matches!(
            reopen(&directory, &name, original),
            Err(Error::UnsafeObject)
        ));
        // Remove only the owned junction itself; never traverse its target.
        std::fs::remove_dir(path.join(&name)).unwrap();
        std::fs::hard_link(path.join("original"), path.join(&name)).unwrap();
        assert!(matches!(
            reopen(&directory, &name, original),
            Err(Error::UnsafeObject)
        ));
    }
    assert_eq!(path.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(path).unwrap();
}
