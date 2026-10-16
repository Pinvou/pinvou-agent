# Controlled compatibility fixtures

The XLSX/ODS pairs contain one sheet, the string `兼容 ✓` and 42.5. They were
constructed independently with Python standard-library `zipfile`, once stored
and once deflated. They are not upstream calamine integration fixtures.

The tiny `.zip` files contain the body `payload`. Fixed local and central
headers select these filename bytes and flags:

| File | Header filename bytes (hex) | UTF8 flag | Unicode path extra |
| --- | --- | --- | --- |
| ascii-control |61736369692e747874|off|none|
| utf8-flag-control |c3a92e747874|on|none|
| utf8-without-flag |c3a92e747874|off|none|
| invalid-utf8-with-flag |ff2e747874|on|none|
| cp437-unambiguous |822e747874|off|none|
| unicode-extra-invalid-crc |626173652e747874|off|version1, CRC0, `renamed.txt`|
| unicode-extra-invalid-utf8 |626173652e747874|off|version1, valid CRC for `base.txt`, bytesff2e747874|

Tests read the actual ZIP APIs and public spreadsheet Reader APIs. They do not
replace production decoders or extract entries to the filesystem. UTF8-flag
ambiguity and invalid Unicode extras intentionally follow upstream zip9.
