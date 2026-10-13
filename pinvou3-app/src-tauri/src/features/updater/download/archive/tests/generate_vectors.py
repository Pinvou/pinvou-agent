"""Independent Python ZIP32 wire fixtures; never imports the Rust implementation."""
import binascii
import argparse
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import zipfile
import zlib


def archive(entries, reverse=False, prefix=b"", gap=b"", comment=b""):
    local = bytearray(prefix)
    central = []
    members = []
    for spec in entries:
        name = spec["name"].encode("ascii")
        data = spec.get("data", b"abc")
        method = spec.get("method", 0)
        version = spec.get("version", 20 if method == 8 else 10)
        flags = spec.get("flags", 8 if spec.get("descriptor") else 0)
        extra = spec.get("extra", b"")
        if method == 8:
            compressor = zlib.compressobj(wbits=-15)
            payload = compressor.compress(data) + compressor.flush()
        else:
            payload = data
        payload += spec.get("tail", b"")
        crc = spec.get("crc", binascii.crc32(data))
        expanded = spec.get("expanded", len(data))
        compressed = len(payload)
        offset = len(local)
        descriptor = spec.get("descriptor", 0)
        values = (0, 0, 0) if descriptor else (crc, compressed, expanded)
        local.extend(struct.pack("<IHHHHHIIIHH", 0x04034B50, version, flags,
                                 method, 0, 0, *values, len(name), len(extra)))
        local.extend(name + extra + payload)
        if descriptor:
            if descriptor == 16:
                local.extend(struct.pack("<I", 0x08074B50))
            local.extend(struct.pack("<III", crc, compressed, expanded))
        attributes = spec.get("attributes", (0o100644 << 16))
        central.append(struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50,
            spec.get("creator", 3) << 8 | 20, version, flags, method, 0, 0, crc,
            compressed, expanded, len(name), len(extra), 0, 0, 0, attributes, offset)
            + name + extra)
        if not spec["name"].endswith("/"):
            members.append({"name": spec["name"], "size": len(data),
                            "sha256": hashlib.sha256(data).hexdigest()})
    local.extend(gap)
    directory = b"".join(reversed(central) if reverse else central)
    end = struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, len(entries), len(entries),
                      len(directory), len(local), len(comment))
    return bytes(local) + directory + end + comment, members


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    if parser.parse_args().check:
        check_vectors()
        return
    vectors = []
    def add(name, entries=None, expected=None, mutate=None, **options):
        raw, members = archive(entries or [{"name": "bin/installer.exe"}], **options)
        if mutate:
            raw = mutate(bytearray(raw))
        vectors.append({"case": name, "hex": bytes(raw).hex(),
                        "sha256": hashlib.sha256(raw).hexdigest(),
                        "error": expected, "members": members})
    add("store")
    add("deflate", [{"name": "data.bin", "method": 8, "data": b"x" * 65537}])
    add("large-deflate", [{"name": "data.bin", "method": 8, "data": b"x" * (33*1024*1024)}])
    for descriptor in (12, 16):
        add(f"descriptor-{descriptor}", [{"name": "data.bin", "method": 8, "descriptor": descriptor}])
    add("utf8-timestamp", [{"name": "data.bin", "flags": 0x800,
                           "extra": struct.pack("<HHBI", 0x5455, 5, 1, 0)}])
    add("directory-after-child", [{"name": "bin/a.exe"}, {"name": "bin/", "data": b"", "attributes": 0o40755 << 16 | 0x10}], reverse=True)
    add("opaque-inner-zip", [{"name": "resource.zip", "data": b"opaque archive data"}])
    add("empty-file", [{"name": "empty.bin", "data": b""}])
    add("central-order", [{"name": "a.bin"}, {"name": "b.bin", "data": b"xyz"}], reverse=True)
    for path in ("../a", "/a", "a//b", "a\\b", "AUX.txt", "a.", "a/CON", "a/./b", "a/../b", "a:"):
        add("path-" + path, [{"name": path}], "ARCHIVE_PATH_REFUSED")
    for names in (("a", "a"), ("a", "A"), ("a", "a/b"), ("a/b", "a"), ("Dir/a", "dir/b")):
        add("collision-" + repr(names), [{"name": name} for name in names], "ARCHIVE_PATH_REFUSED")
    add("unix-link", [{"name": "link", "attributes": 0o120777 << 16}], "ARCHIVE_FILE_TYPE_REFUSED")
    add("unix-device", [{"name": "device", "attributes": 0o020600 << 16}], "ARCHIVE_FILE_TYPE_REFUSED")
    add("setuid", [{"name": "tool", "attributes": 0o104755 << 16}], "ARCHIVE_FILE_TYPE_REFUSED")
    add("dos-device", [{"name": "device", "creator": 0, "attributes": 0x40}], "ARCHIVE_FILE_TYPE_REFUSED")
    add("dos-reparse", [{"name": "reparse", "creator": 0, "attributes": 0x400}], "ARCHIVE_FILE_TYPE_REFUSED")
    add("unix-type-name-mismatch", [{"name": "dir/", "data": b"", "attributes": 0o100644 << 16}], "ARCHIVE_FILE_TYPE_REFUSED")
    add("encrypted", [{"name": "a", "flags": 1}], "ARCHIVE_FORMAT_UNSUPPORTED")
    add("zip64-extra", [{"name": "a", "extra": struct.pack("<HH", 1, 0)}], "ARCHIVE_FORMAT_UNSUPPORTED")
    add("unicode-rewrite-extra", [{"name": "a", "extra": struct.pack("<HH", 0x7075, 0)}], "ARCHIVE_FORMAT_UNSUPPORTED")
    add("unknown-method", [{"name": "a", "method": 99}], "ARCHIVE_FORMAT_UNSUPPORTED")
    add("compressed-tail", [{"name": "a", "method": 8, "tail": b"hidden"}], "ARCHIVE_COMPRESSED_TAIL_REFUSED")
    for n in (2, 4):
        add(f"expanded-{n}", [{"name": "a", "method": 8, "expanded": n}], "ARCHIVE_ENTRY_BYTES_INVALID")
    add("bad-crc", [{"name": "a", "method": 8, "crc": 123}], "ARCHIVE_ENTRY_BYTES_INVALID")
    add("later-bad-entry", [{"name": "good"}, {"name": "bad", "method": 8, "crc": 123}], "ARCHIVE_ENTRY_BYTES_INVALID")
    add("prefix", prefix=b"prefix", expected="ARCHIVE_STRUCTURE_INVALID")
    add("gap", gap=b"gap", expected="ARCHIVE_STRUCTURE_INVALID")
    def change_at(offset, data):
        def mutate(raw):
            raw[offset:offset+len(data)] = data
            return raw
        return mutate
    add("local-name-mismatch", expected="ARCHIVE_STRUCTURE_INVALID", mutate=change_at(30, b"x"))
    add("local-time-mismatch", expected="ARCHIVE_STRUCTURE_INVALID", mutate=change_at(10, b"\x01\x00"))
    add("descriptor-mismatch", [{"name": "a", "method": 8, "descriptor": 16}], "ARCHIVE_STRUCTURE_INVALID", mutate=change_at(40, b"\x99"))
    add("split", expected="ARCHIVE_FORMAT_UNSUPPORTED", mutate=change_at(-18, b"\x01\x00"))
    add("zip64-sentinel", expected="ARCHIVE_FORMAT_UNSUPPORTED", mutate=change_at(-14, b"\xff\xff\xff\xff"))
    add("ambiguous-eocd", comment=struct.pack("<IHHHHIIH", 0x06054B50,0,0,0,0,0,0,0), expected="ARCHIVE_STRUCTURE_INVALID")
    raw, members = archive([{"name": "large.bin", "data": b"x" * (33*1024*1024)}])
    start = 30 + len("large.bin")
    vectors.append({"case": "large-stored", "headerHex": raw[:start].hex(),
                    "tailHex": raw[start+33*1024*1024:].hex(), "repeat": 33*1024*1024,
                    "sha256": hashlib.sha256(raw).hexdigest(), "error": None,
                    "members": members})
    path = Path(__file__).resolve().parent / "fixtures/zip-vectors.json"
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(vectors, indent=2) + "\n", encoding="utf-8")
    print(f"Generated {len(vectors)} independently hashed ZIP vectors")


def check_vectors():
    path = Path(__file__).resolve().parent / "fixtures/zip-vectors.json"
    vectors = json.loads(path.read_text(encoding="utf-8"))
    for vector in vectors:
        with tempfile.TemporaryFile() as source:
            if "hex" in vector:
                source.write(bytes.fromhex(vector["hex"]))
            else:
                source.write(bytes.fromhex(vector["headerHex"]))
                remaining = vector["repeat"]
                while remaining:
                    block = b"x" * min(65536, remaining)
                    source.write(block)
                    remaining -= len(block)
                source.write(bytes.fromhex(vector["tailHex"]))
            source.seek(0)
            digest = hashlib.sha256()
            while block := source.read(65536):
                digest.update(block)
            if digest.hexdigest() != vector["sha256"]:
                raise RuntimeError("fixture container hash mismatch: " + vector["case"])
            if vector["error"] is None:
                source.seek(0)
                with zipfile.ZipFile(source) as archive_file:
                    members = {item["name"]: item for item in vector["members"]}
                    files = [item for item in archive_file.infolist() if not item.is_dir()]
                    if len(files) != len(members):
                        raise RuntimeError("fixture member count mismatch")
                    for item in files:
                        expected = members[item.filename]
                        digest = hashlib.sha256()
                        size = 0
                        with archive_file.open(item) as member:
                            while block := member.read(65536):
                                digest.update(block)
                                size += len(block)
                        if size != expected["size"] or digest.hexdigest() != expected["sha256"]:
                            raise RuntimeError("fixture member bytes mismatch")
    print(f"Python verified {len(vectors)} container hashes and all positive ZIP inventories")


if __name__ == "__main__":
    main()
