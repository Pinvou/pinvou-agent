# Windows Tesseract OCR Runtime

## Source

- Source directory: `C:\Program Files\Tesseract-OCR`
- Runtime version: `tesseract v5.5.0.20241111`
- Leptonica version reported by runtime: `1.85.0`

This directory is the controlled Windows OCR runtime bundled with pinvou. It is installed to `{install_dir}/tesseract` by the Windows MSI.

## Included Files

- `tesseract.exe`
- Runtime DLLs required by `tesseract.exe`
- `tessdata/chi_sim.traineddata`
- `tessdata/eng.traineddata`
- `LICENSE`
- `UPSTREAM-README.md`

## Excluded Files

The original installation contains training, diagnostics, documentation and extra language files that are not needed for pinvou's scanned-PDF OCR path. They are intentionally excluded from the MSI:

- Training and test tools such as `lstmtraining.exe`, `lstmeval.exe`, `cntraining.exe`, `mftraining.exe`, `classifier_tester.exe`
- Utility tools not used by pinvou OCR, such as `combine_tessdata.exe`, `wordlist2dawg.exe`, `dawg2wordlist.exe`
- HTML man pages and other local command documentation
- Extra language data such as `chi_sim_vert.traineddata` and `osd.traineddata`
- Java ScrollView jars and helper assets

pinvou only uses `tesseract.exe <image> - -l chi_sim+eng --tessdata-dir <install_dir>/tesseract/tessdata` for scanned PDF fallback OCR.

## License

The upstream license is copied as `LICENSE`. The upstream README is copied as `UPSTREAM-README.md` for source and runtime context.

## Size Notes

The initial bundled runtime is about 78.4 MB before MSI compression, with 62 files in this directory. Future trimming should be validated by running `tesseract.exe --version`, `tesseract.exe --list-langs --tessdata-dir tessdata`, and a scanned-PDF OCR smoke test after each removed DLL or data file.
