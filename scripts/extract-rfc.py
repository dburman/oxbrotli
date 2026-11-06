#!/usr/bin/env python3
"""Extracts the tables oxbrotli takes from RFC 7932, so nothing is typed in by hand.

    scripts/extract-rfc.py path/to/rfc7932.txt

Reads a local copy of the RFC (no network access; download it from
https://www.rfc-editor.org/rfc/rfc7932.txt) and writes:

- src/context_lut.rs: Lut0, Lut1 and Lut2 from section 7.1.
- src/dictionary.bin: the static dictionary from Appendix A.
- src/dictionary_tables.rs: NDBITS from Appendix A and the 121 transforms of Appendix B.

It checks each table's length and CRC-32 against the values the RFC lists (Appendix B gives
one for the transforms serialized as it describes), and fails without writing anything if
one doesn't match.
"""

import pathlib
import re
import sys
import zlib

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Section 7.1 lists these lengths and CRC-32 values for the tables as bytes.
LUT_CRC = {"Lut0": 0x8E91EFB7, "Lut1": 0xD01A32F4, "Lut2": 0x0DD7A0D6}

# Appendix A: the dictionary's length and CRC-32. Appendix B: the transforms' serialized
# length and CRC-32.
DICT_LEN, DICT_CRC = 122_784, 0x5136CB04
TRANSFORMS_LEN, TRANSFORMS_CRC = 648, 0x3D965F81

# Appendix B's elementary transforms by the value it gives each one.
KINDS = (
    ["Identity", "FermentFirst", "FermentAll"]
    + [f"OmitFirst{k}" for k in range(1, 10)]
    + [f"OmitLast{k}" for k in range(1, 10)]
)


def pages_removed(text):
    """The RFC text without page footers, headers and form feeds."""
    out = []
    for line in text.splitlines():
        if line.startswith("Alakuijala & Szabadka") or line.startswith("RFC 7932  "):
            continue
        out.append(line.replace("\f", ""))
    return "\n".join(out)


def lut(text, name):
    """The 256 values of `name` (Lut0, Lut1, Lut2) from section 7.1."""
    start = text.index(f"      {name} :=")
    rest = text[start + len(name) + 9 :]
    values = []
    for number in re.finditer(r"\d+|[A-Za-z]", rest):
        if not number.group().isdigit():
            break
        values.append(int(number.group()))
        if len(values) == 256:
            break
    if len(values) != 256:
        sys.exit(f"{name}: found {len(values)} values, expected 256")
    crc = zlib.crc32(bytes(values))
    if crc != LUT_CRC[name]:
        sys.exit(f"{name}: CRC-32 {crc:#010x}, the RFC says {LUT_CRC[name]:#010x}")
    return values


def dictionary(text):
    """The DICT array of Appendix A."""
    start = text.index("Appendix A.  Static Dictionary Data", text.index("Appendix A.") + 1)
    end = text.index("The number of words for each length", start)
    hexdigits = "".join(re.findall(r"^\s+([0-9a-f]{64}|[0-9a-f]+)$", text[start:end], re.M))
    data = bytes.fromhex(hexdigits)
    crc = zlib.crc32(data)
    if len(data) != DICT_LEN or crc != DICT_CRC:
        sys.exit(f"dictionary: {len(data)} bytes, CRC-32 {crc:#010x}; the RFC says "
                 f"{DICT_LEN} bytes, {DICT_CRC:#010x}")
    return data


def ndbits(text):
    """NDBITS[0..=24] from Appendix A."""
    start = text.index("NDBITS :=")
    values = [int(v) for v in re.findall(r"\d+", text[start + 9 : start + 400])[:25]]
    if len(values) != 25:
        sys.exit(f"NDBITS: found {len(values)} values, expected 25")
    return values


def c_string(literal):
    """The bytes of a C string literal's contents (Appendix B's escapes: \\", \\n, \\t, \\xNN)."""
    return literal.encode("latin-1").decode("unicode_escape").encode("latin-1")


def transforms(text):
    """The 121 (prefix, kind, suffix) transforms of Appendix B, checked against its CRC-32."""
    start = text.index("Appendix B.  List of Word Transformations", text.index("Appendix B.") + 1)
    end = text.index("Appendix C.", start)
    rows = re.findall(r'^\s+(\d+)\s+"((?:[^"\\]|\\.)*)"\s+(\w+)\s+"((?:[^"\\]|\\.)*)"\s*$',
                      text[start:end], re.M)
    out = []
    for i, (ident, prefix, kind, suffix) in enumerate(rows):
        if int(ident) != i:
            sys.exit(f"transforms: row {i} has ID {ident}")
        if kind not in KINDS:
            sys.exit(f"transforms: unknown transform {kind}")
        out.append((c_string(prefix), kind, c_string(suffix)))
    if len(out) != 121:
        sys.exit(f"transforms: found {len(out)}, expected 121")
    serial = b"".join(p + b"\0" + bytes([KINDS.index(k)]) + x + b"\0" for p, k, x in out)
    crc = zlib.crc32(serial)
    if len(serial) != TRANSFORMS_LEN or crc != TRANSFORMS_CRC:
        sys.exit(f"transforms: {len(serial)} bytes, CRC-32 {crc:#010x}; the RFC says "
                 f"{TRANSFORMS_LEN} bytes, {TRANSFORMS_CRC:#010x}")
    return out


def rust_bytes(b):
    """A Rust byte string literal for `b`."""
    out = []
    for c in b:
        if c == 0x22:
            out.append('\\"')
        elif c == 0x5C:
            out.append("\\\\")
        elif 0x20 <= c < 0x7F:
            out.append(chr(c))
        elif c == 0x0A:
            out.append("\\n")
        elif c == 0x09:
            out.append("\\t")
        else:
            out.append(f"\\x{c:02x}")
    return 'b"' + "".join(out) + '"'


def rust_kind(kind):
    for name in ("OmitFirst", "OmitLast"):
        if kind.startswith(name):
            return f"Kind::{name}({kind[len(name):]})"
    return f"Kind::{kind}"


def rust_array(name, values, doc):
    rows = []
    for i in range(0, len(values), 16):
        rows.append("    " + ", ".join(str(v) for v in values[i : i + 16]) + ",")
    return f"/// {doc}\n#[rustfmt::skip]\npub(crate) static {name}: [u8; {len(values)}] = [\n" + "\n".join(rows) + "\n];\n"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    text = pages_removed(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
    tables = {name: lut(text, name) for name in ("Lut0", "Lut1", "Lut2")}
    out = [
        "//! The context lookup tables of RFC 7932 §7.1.",
        "//!",
        "//! Generated by `scripts/extract-rfc.py` from the RFC text; don't edit by hand. Each",
        "//! table's CRC-32 matches the value the RFC lists (checked by the script and by a test).",
        "",
    ]
    for name, doc in [
        ("Lut0", "Lut0: the UTF8 context's part from the last byte."),
        ("Lut1", "Lut1: the UTF8 context's part from the byte before it."),
        ("Lut2", "Lut2: the Signed context's parts, from either byte."),
    ]:
        out.append(rust_array(name.upper(), tables[name], doc))
    dict_bytes = dictionary(text)
    bits = ndbits(text)
    tlist = transforms(text)

    path = ROOT / "src" / "context_lut.rs"
    path.write_text("\n".join(out))
    print(f"wrote {path.relative_to(ROOT)}: Lut0, Lut1, Lut2 (CRC-32 match the RFC)")

    path = ROOT / "src" / "dictionary.bin"
    path.write_bytes(dict_bytes)
    print(f"wrote {path.relative_to(ROOT)}: {len(dict_bytes)} bytes, CRC-32 {DICT_CRC:#010x}")

    lines = [
        "//! NDBITS (RFC 7932 Appendix A) and the word transforms (Appendix B).",
        "//!",
        "//! Generated by `scripts/extract-rfc.py` from the RFC text; don't edit by hand. The",
        "//! transforms, serialized as Appendix B describes, match its length and CRC-32.",
        "",
        "use crate::dictionary::Kind;",
        "",
        "/// log2 of the number of dictionary words of each length 0..=24 (0: no words).",
        "#[rustfmt::skip]",
        "pub(crate) const NDBITS: [u8; 25] = [" + ", ".join(str(b) for b in bits) + "];",
        "",
        "/// The 121 transforms: prefix, elementary transform, suffix.",
        "#[rustfmt::skip]",
        "pub(crate) static TRANSFORMS: [(&[u8], Kind, &[u8]); 121] = [",
    ]
    for i, (prefix, kind, suffix) in enumerate(tlist):
        lines.append(f"    ({rust_bytes(prefix)}, {rust_kind(kind)}, {rust_bytes(suffix)}), // {i}")
    lines.append("];")
    path = ROOT / "src" / "dictionary_tables.rs"
    path.write_text("\n".join(lines) + "\n")
    print(f"wrote {path.relative_to(ROOT)}: NDBITS, 121 transforms (CRC-32 {TRANSFORMS_CRC:#010x})")


if __name__ == "__main__":
    main()
