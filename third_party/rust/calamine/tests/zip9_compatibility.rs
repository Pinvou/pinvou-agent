//! Pinvou compatibility regressions for the locally patched calamine/zip9 pair.
//! Fixtures are minimal independently constructed OOXML/ODS and fixed ZIP bytes.
use calamine::{open_workbook_auto, Data, Reader};
use std::io::{Cursor, Read};

fn read_sheet(name: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let mut workbook = open_workbook_auto(path).unwrap();
    assert_eq!(workbook.sheet_names(), ["Sheet1"]);
    let range = workbook.worksheet_range("Sheet1").unwrap();
    assert_eq!(
        range.get_value((0, 0)),
        Some(&Data::String("兼容 ✓".into()))
    );
    assert_eq!(range.get_value((0, 1)), Some(&Data::Float(42.5)));
}

#[test]
fn xlsx_stored() {
    read_sheet("stored.xlsx");
}
#[test]
fn xlsx_deflated() {
    read_sheet("deflated.xlsx");
}
#[test]
fn ods_stored() {
    read_sheet("stored.ods");
}
#[test]
fn ods_deflated() {
    read_sheet("deflated.ods");
}

fn archive(bytes: &'static [u8]) -> zip::ZipArchive<Cursor<&'static [u8]>> {
    zip::ZipArchive::new(Cursor::new(bytes)).unwrap()
}

#[test]
fn normal_unicode_entry_name_roundtrips() {
    let mut zip = archive(include_bytes!("fixtures/utf8-flag-control.zip"));
    let names = zip
        .file_names()
        .collect::<zip::result::ZipResult<Vec<_>>>()
        .unwrap();
    assert_eq!(names, ["é.txt"]);
    let mut body = String::new();
    zip.by_name("é.txt")
        .unwrap()
        .read_to_string(&mut body)
        .unwrap();
    assert_eq!(body, "payload");
}

#[test]
fn unflagged_valid_utf8_uses_zip9_default_decoder() {
    let mut zip = archive(include_bytes!("fixtures/utf8-without-flag.zip"));
    assert_eq!(zip.by_index(0).unwrap().flags().as_u16() & 0x800, 0);
    assert_eq!(zip.by_index(0).unwrap().name().unwrap(), "é.txt");
    assert!(zip.by_name("é.txt").is_ok());
}

#[test]
fn flagged_invalid_utf8_uses_zip9_cp437_fallback() {
    let mut zip = archive(include_bytes!("fixtures/invalid-utf8-with-flag.zip"));
    assert_ne!(zip.by_index(0).unwrap().flags().as_u16() & 0x800, 0);
    assert_eq!(zip.by_index(0).unwrap().name().unwrap(), "\u{00a0}.txt");
}

#[test]
fn invalid_unicode_extra_fields_fall_back_to_header() {
    for bytes in [
        include_bytes!("fixtures/unicode-extra-invalid-crc.zip").as_slice(),
        include_bytes!("fixtures/unicode-extra-invalid-utf8.zip").as_slice(),
    ] {
        let mut zip = archive(bytes);
        assert_eq!(zip.by_index(0).unwrap().name().unwrap(), "base.txt");
        let mut body = String::new();
        zip.by_name("base.txt")
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        assert_eq!(body, "payload");
    }
}

#[test]
fn cp437_decoding_does_not_claim_decoded_name_lookup_support() {
    let mut zip = archive(include_bytes!("fixtures/cp437-unambiguous.zip"));
    assert_eq!(zip.by_index(0).unwrap().name().unwrap(), "é.txt");
    assert!(matches!(
        zip.by_name("é.txt"),
        Err(zip::result::ZipError::FileNotFound)
    ));
    let mut ascii = archive(include_bytes!("fixtures/ascii-control.zip"));
    assert!(ascii.by_name("ascii.txt").is_ok());
}
