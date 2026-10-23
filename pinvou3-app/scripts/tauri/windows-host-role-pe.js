// Read-only build gate for the two native upgrade roles, before any execution.
// This parses bounded PE metadata; it never loads an image or a runtime DLL.
const fs = require("node:fs");
const { refuse } = require("./windows-host-profile.js");
const { noReparseAncestors } = require("./windows-host-manifest.js");

const MAXIMUM_SECTIONS = 96; // Windows PE loader limit.
const MAXIMUM_IMPORTS = 4096;
const MAXIMUM_NAME_BYTES = 256;
const MAXIMUM_METADATA_BYTES = 8 * 1024 * 1024;
const DYNAMIC_CRT = /^(?:(?:vcruntime|msvcp|msvcr|msvcrt|concrt|vcomp|vcamp|vccorlib)\d[^.]*|msvcp_win|ucrtbase(?:d)?|msvcrt(?:d)?|api-ms-win-crt-[a-z0-9-]+)\.dll$/iu;

function invalid() { refuse("HOST_BUILD_ROLE_IMAGE_INVALID"); }
function sameFile(left, right) {
  return left.dev === right.dev && left.ino === right.ino && left.size === right.size
    && left.birthtimeNs === right.birthtimeNs && left.mtimeNs === right.mtimeNs && left.ctimeNs === right.ctimeNs;
}

function assertHostRoleImage(image) {
  let fd;
  try {
    noReparseAncestors(image);
    const named = fs.lstatSync(image, { bigint: true });
    if (!named.isFile() || named.isSymbolicLink() || named.nlink !== 1n) invalid();
    fd = fs.openSync(image, "r");
    const before = fs.fstatSync(fd, { bigint: true });
    if (!sameFile(before, named) || before.size < 64n || before.size > BigInt(Number.MAX_SAFE_INTEGER)) invalid();
    const fileSize = Number(before.size);
    let observedBytes = 0;
    function read(offset, length) {
      if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isInteger(length) || length < 1
          || length > 4096 || offset > fileSize - length
          || observedBytes > MAXIMUM_METADATA_BYTES - length) invalid();
      observedBytes += length;
      const bytes = Buffer.alloc(length);
      let received = 0;
      while (received < length) {
        const count = fs.readSync(fd, bytes, received, length - received, offset + received);
        if (!count) invalid();
        received += count;
      }
      return bytes;
    }
    const dos = read(0, 64);
    if (dos.readUInt16LE(0) !== 0x5a4d) invalid();
    const peOffset = dos.readUInt32LE(60);
    if (peOffset < 64) invalid();
    const coff = read(peOffset, 24);
    if (coff.readUInt32LE(0) !== 0x4550 || coff.readUInt16LE(4) !== 0x8664) invalid();
    const sectionCount = coff.readUInt16LE(6), optionalSize = coff.readUInt16LE(20);
    if (sectionCount < 1 || sectionCount > MAXIMUM_SECTIONS || optionalSize < 200 || optionalSize > 4096) invalid();
    const optional = read(peOffset + 24, optionalSize);
    if (optional.readUInt16LE(0) !== 0x20b) invalid();
    const directories = optional.readUInt32LE(108);
    if (directories < 11 || directories > 16 || optionalSize < 112 + directories * 8) invalid();
    const headerSize = optional.readUInt32LE(60), imageSize = optional.readUInt32LE(56);
    const sectionOffset = peOffset + 24 + optionalSize;
    if (headerSize < sectionOffset + sectionCount * 40 || headerSize > fileSize || imageSize < headerSize) invalid();
    const sectionBytes = read(sectionOffset, sectionCount * 40);
    const sections = [];
    for (let index = 0; index < sectionCount; index++) {
      const offset = index * 40, rva = sectionBytes.readUInt32LE(offset + 12);
      const virtualSize = sectionBytes.readUInt32LE(offset + 8);
      const rawSize = sectionBytes.readUInt32LE(offset + 16), raw = sectionBytes.readUInt32LE(offset + 20);
      const mappedSize = Math.max(virtualSize, rawSize);
      if (rva < headerSize || rva > imageSize - mappedSize
          || (rawSize && (raw < headerSize || raw > fileSize - rawSize))) invalid();
      for (const prior of sections) {
        if (mappedSize && prior.mappedSize && rva < prior.rva + prior.mappedSize && prior.rva < rva + mappedSize) invalid();
        if (rawSize && prior.rawSize && raw < prior.raw + prior.rawSize && prior.raw < raw + rawSize) invalid();
      }
      sections.push({ rva, mappedSize, raw, rawSize });
    }
    function mapping(rva, length) {
      if (!Number.isInteger(rva) || rva < 0 || !Number.isInteger(length) || length < 1 || rva > 0xffffffff - length + 1) invalid();
      if (rva < headerSize && length <= headerSize - rva) return { offset: rva, available: headerSize - rva };
      const section = sections.find(value => rva >= value.rva && rva - value.rva < value.rawSize
        && length <= value.rawSize - (rva - value.rva));
      if (!section) invalid();
      return { offset: section.raw + rva - section.rva, available: section.rawSize - (rva - section.rva) };
    }
    function directory(index) {
      if (index >= directories) return null;
      const offset = 112 + index * 8, rva = optional.readUInt32LE(offset), size = optional.readUInt32LE(offset + 4);
      if (!rva && !size) return null;
      if (!rva || !size) invalid();
      const mapped = mapping(rva, size);
      return { rva, size, offset: mapped.offset };
    }
    const load = directory(10);
    if (!load || load.size < 80) invalid();
    const config = read(load.offset, 80), configSize = config.readUInt32LE(0);
    if (configSize < 80 || configSize > load.size) invalid();
    if (config.readUInt16LE(78) !== 0x800) refuse("HOST_BUILD_ROLE_LOADER_UNSAFE");
    function libraryName(rva) {
      const mapped = mapping(rva, 1);
      const bytes = read(mapped.offset, Math.min(MAXIMUM_NAME_BYTES, mapped.available));
      const end = bytes.indexOf(0);
      if (end < 1) invalid();
      for (const value of bytes.subarray(0, end)) if (value < 0x21 || value > 0x7e) invalid();
      const name = bytes.subarray(0, end).toString("ascii");
      if (!/^[a-z0-9_.-]+\.dll$/iu.test(name)) invalid();
      if (DYNAMIC_CRT.test(name)) refuse("HOST_BUILD_ROLE_RUNTIME_IMPORT");
      return name;
    }
    function imports(index, descriptorSize, delayed) {
      const table = directory(index), names = [];
      if (!table) return names;
      const maximum = Math.min(Math.floor(table.size / descriptorSize), MAXIMUM_IMPORTS + 1);
      for (let index = 0; index < maximum; index++) {
        const bytes = read(table.offset + index * descriptorSize, descriptorSize);
        if (bytes.every(value => value === 0)) return names;
        if (index === MAXIMUM_IMPORTS) invalid();
        // PE32+ images produced by the MSVC toolchain use RVA-based delay
        // descriptors. Legacy VA/unknown-attribute forms are not this format.
        if (delayed && bytes.readUInt32LE(0) !== 1) invalid();
        names.push(libraryName(bytes.readUInt32LE(delayed ? 4 : 12)));
      }
      invalid(); // A missing terminator is not an empty dependency set.
    }
    const normal = imports(1, 20, false), delayed = imports(13, 32, true);
    const after = fs.fstatSync(fd, { bigint: true }), current = fs.lstatSync(image, { bigint: true });
    if (!current.isFile() || current.isSymbolicLink() || current.nlink !== 1n
        || !sameFile(before, after) || !sameFile(after, current)) invalid();
    return { dependentLoadFlags: 0x800, imports: normal, delayImports: delayed };
  } catch (error) {
    if (error instanceof Error && /^HOST_BUILD_ROLE_(IMAGE_INVALID|LOADER_UNSAFE|RUNTIME_IMPORT)$/u.test(error.message)) throw error;
    invalid();
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

module.exports = { assertHostRoleImage };
