use crate::{Error, SignatureEvidence};
use std::{fs::File, sync::atomic::AtomicBool};
pub(crate) fn lock(_: &File) -> Result<File, Error> {
    Err(Error::Unsupported)
}
pub(crate) fn inspect(_: &File, _: &AtomicBool) -> Result<Vec<SignatureEvidence>, Error> {
    Err(Error::Unsupported)
}
pub struct ConsoleCancellation;
impl ConsoleCancellation {
    pub fn install() -> Result<Self, Error> {
        Err(Error::Unsupported)
    }
    pub fn flag(&self) -> &AtomicBool {
        static FLAG: AtomicBool = AtomicBool::new(false);
        &FLAG
    }
}
#[test]
fn unsupported_does_not_touch_a_path() {
    let expected = crate::ExpectedFile::new(3, &"a".repeat(64)).unwrap();
    assert!(matches!(
        crate::observe_path(
            std::path::Path::new("unused"),
            &expected,
            &AtomicBool::new(false)
        ),
        Err(Error::Unsupported)
    ));
}
