"""Independent Python implementation of edda-canon-v1 canonical JSON.

Canonical form (pinned by spec/events/canonical-v1.json and the Rust
``canonical_byte_vectors_match_before_hashing`` test):

- object keys sorted lexicographically by Unicode code point order
  (== UTF-8 byte order), recursive; arrays keep order; compact separators.
- strings are decoded and re-emitted per serde_json escaping rules: ``"``
  and ``\\`` escaped, C0 controls as \\b \\t \\n \\f \\r or ``\\u00xx``
  (lowercase hex), everything else (incl. all non-ASCII) emitted raw.
- numbers are parsed and re-emitted per serde_json (zmij) semantics:
  * integer-form lexemes that fit u64/i64 are emitted as exact integers
    (u64 max and i64 min survive); larger integer lexemes fall back to f64.
  * everything else is an f64, emitted as shortest-roundtrip digits with
    plain decimal notation when the decimal exponent is in [-5, 15]
    (always with at least one fractional digit: 1.0, -0.0), otherwise
    exponential with explicit sign and no zero padding: 1e+30, 1e-7.
  * non-finite results (e.g. 1e999) raise an error — serde_json refuses
    them too.

Hash rule (docs/reference/ledger-event-spec.md): event hash = SHA-256 of the
canonical JSON of the event with top-level ``hash``, ``digests`` and
``schema_version`` removed.

Scope honesty: arbitrary-precision numbers beyond u64/i64 integer form are
NOT preserved (they degrade to f64, matching serde_json); the integer lexeme
"-0" normalizes to "0", while float "-0.0" stays "-0.0".
"""

from __future__ import annotations

import hashlib
import json
import math
import re

WHITESPACE = " \t\n\r"

EVENT_HASH_EXCLUDED_KEYS = frozenset({"hash", "digests", "schema_version"})

JSON_NUMBER = re.compile(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$")
INTEGER_LEXEME = re.compile(r"^-?(0|[1-9][0-9]*)$")
_I64_MIN = -(2**63)
_U64_MAX = 2**64 - 1


def _skip_ws(s: str, i: int) -> int:
    while i < len(s) and s[i] in WHITESPACE:
        i += 1
    return i


def _decode_string(s: str, i: int) -> tuple[str, int]:
    """Parse a JSON string token starting at s[i] == '"'; return (decoded, next)."""
    if s[i] != '"':
        raise ValueError(f"expected string at {i}")
    out: list[str] = []
    j = i + 1
    while True:
        if j >= len(s):
            raise ValueError(f"unterminated string at {i}")
        c = s[j]
        if c == '"':
            return "".join(out), j + 1
        if c == "\\":
            e = s[j + 1]
            if e == '"':
                out.append('"')
                j += 2
            elif e == "\\":
                out.append("\\")
                j += 2
            elif e == "/":
                out.append("/")
                j += 2
            elif e == "b":
                out.append("\b")
                j += 2
            elif e == "f":
                out.append("\f")
                j += 2
            elif e == "n":
                out.append("\n")
                j += 2
            elif e == "r":
                out.append("\r")
                j += 2
            elif e == "t":
                out.append("\t")
                j += 2
            elif e == "u":
                hex4 = s[j + 2 : j + 6]
                if not re.fullmatch(r"[0-9a-fA-F]{4}", hex4):
                    raise ValueError(f"bad \\u escape at {j}")
                cp = int(hex4, 16)
                if 0xD800 <= cp <= 0xDBFF:
                    if s[j + 6 : j + 8] != "\\u":
                        raise ValueError(f"lone high surrogate at {j}")
                    lo = int(s[j + 8 : j + 12], 16)
                    if not 0xDC00 <= lo <= 0xDFFF:
                        raise ValueError(f"lone high surrogate at {j}")
                    out.append(chr(0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)))
                    j += 12
                elif 0xDC00 <= cp <= 0xDFFF:
                    raise ValueError(f"lone low surrogate at {j}")
                else:
                    out.append(chr(cp))
                    j += 6
            else:
                raise ValueError(f"bad escape at {j}")
        elif c < " ":
            raise ValueError(f"raw control char in string at {j}")
        else:
            out.append(c)
            j += 1


_SIMPLE_ESCAPES = {'"': '\\"', "\\": "\\\\", "\b": "\\b", "\t": "\\t", "\n": "\\n", "\f": "\\f", "\r": "\\r"}


def _emit_string(decoded: str) -> str:
    """Re-emit a decoded string per serde_json escaping rules."""
    parts = ['"']
    for ch in decoded:
        esc = _SIMPLE_ESCAPES.get(ch)
        if esc is not None:
            parts.append(esc)
        elif ord(ch) < 0x20:
            parts.append("\\u%04x" % ord(ch))
        else:
            parts.append(ch)
    parts.append('"')
    return "".join(parts)


def _zmij_format(digits: str, dec_exp: int) -> str:
    """zmij decimal/exponential layout from shortest digits."""
    n = len(digits)
    if -5 <= dec_exp <= 15:
        if n - 1 <= dec_exp:
            # 1234e7 -> 12340000000.0 ; 1.0
            return digits + "0" * (dec_exp - (n - 1)) + ".0"
        if dec_exp >= 0:
            # 1234e-2 -> 12.34
            return digits[: dec_exp + 1] + "." + digits[dec_exp + 1 :]
        # 1234e-6 -> 0.001234
        return "0." + "0" * (-dec_exp - 1) + digits
    # exponential: 1e+30, 1.234e+33, 1e-7
    mant = digits[0] + "." + digits[1:] if n > 1 else digits
    sign = "+" if dec_exp >= 0 else "-"
    return f"{mant}e{sign}{abs(dec_exp)}"


def _format_f64(f: float) -> str:
    """Format an f64 per serde_json's (zmij) shortest-roundtrip emitter."""
    if not math.isfinite(f):
        raise ValueError("number out of range (non-finite)")
    if f == 0.0:
        return "-0.0" if math.copysign(1.0, f) < 0 else "0.0"
    # shortest digits + decimal exponent via repr, normalized:
    text = repr(f)
    if "e" in text or "E" in text:
        mant, _, exp = text.lower().partition("e")
        e = int(exp)
    else:
        mant, e = text, 0
    sign = ""
    if mant.startswith("-"):
        sign, mant = "-", mant[1:]
    int_part, _, frac_part = mant.partition(".")
    digits_all = int_part + frac_part
    first_sig = min(i for i, ch in enumerate(digits_all) if ch != "0")
    point_pos = len(int_part)  # position of the decimal point in digits_all
    dec_exp = point_pos - first_sig - 1 + e  # exponent form adds to it
    digits = digits_all[first_sig:].rstrip("0") or "0"
    return sign + _zmij_format(digits, dec_exp)


def _canonical_number(lexeme: str) -> str:
    """Canonical number emission from a raw lexeme (serde_json parse semantics)."""
    if not JSON_NUMBER.fullmatch(lexeme):
        raise ValueError(f"invalid JSON number: {lexeme}")
    if INTEGER_LEXEME.fullmatch(lexeme):
        v = int(lexeme)
        if _I64_MIN <= v <= _U64_MAX:
            return str(v)  # "-0" -> "0", exact u64/i64
        # serde_json falls back to f64 for integer lexemes beyond u64/i64
    return _format_f64(float(lexeme))


def _parse_value(s: str, i: int) -> tuple[dict | str, int]:
    """Return ('object', [(decoded_key, canonical_text), ...]) /
    ('array', [texts]) / pre-rendered scalar text."""
    i = _skip_ws(s, i)
    c = s[i] if i < len(s) else ""
    if c == "{":
        entries: dict[str, str] = {}
        i = _skip_ws(s, i + 1)
        if s[i] == "}":
            return ("object", list(entries.items())), i + 1
        while True:
            i = _skip_ws(s, i)
            if s[i] != '"':
                raise ValueError(f"expected object key at {i}")
            key, i = _decode_string(s, i)
            i = _skip_ws(s, i)
            if s[i] != ":":
                raise ValueError(f"expected ':' at {i}")
            value, i = _parse_value(s, i + 1)
            entries[key] = _render(value)  # last key wins, like serde_json
            i = _skip_ws(s, i)
            if s[i] == ",":
                i += 1
                continue
            if s[i] == "}":
                return ("object", list(entries.items())), i + 1
            raise ValueError(f"expected ',' or '}}' at {i}")
    if c == "[":
        items: list[str] = []
        i = _skip_ws(s, i + 1)
        if s[i] == "]":
            return ("array", items), i + 1
        while True:
            value, i = _parse_value(s, i)
            items.append(_render(value))
            i = _skip_ws(s, i)
            if s[i] == ",":
                i += 1
                continue
            if s[i] == "]":
                return ("array", items), i + 1
            raise ValueError(f"expected ',' or ']' at {i}")
    if c == '"':
        decoded, i = _decode_string(s, i)
        return _emit_string(decoded), i
    # number / true / false / null lexeme
    j = i
    while j < len(s) and s[j] not in WHITESPACE and s[j] not in ",}]":
        j += 1
    if j == i:
        raise ValueError(f"unexpected char at {i}")
    lexeme = s[i:j]
    if lexeme in ("true", "false", "null"):
        return lexeme, j
    return _canonical_number(lexeme), j


def _render(v: dict | str, excluded_top_level: frozenset[str] | None = None, is_top: bool = True) -> str:
    if isinstance(v, str):  # pre-rendered scalar
        return v
    kind, body = v
    if kind == "object":
        parts = []
        for k, canonical_val in sorted(body, key=lambda kv: kv[0].encode("utf-8")):
            # Exclusions apply only at depth 0 (event hash rule).
            if is_top and excluded_top_level and k in excluded_top_level:
                continue
            parts.append(_emit_string(k) + ":" + canonical_val)
        return "{" + ",".join(parts) + "}"
    return "[" + ",".join(body) + "]"


def canonicalize_text(raw_json: str, excluded_top_level: frozenset[str] = frozenset()) -> str:
    """Canonicalize raw JSON text; top-level keys in excluded_top_level are dropped.

    Number and string emission follow serde_json (zmij) semantics per
    spec/events/canonical-v1.json.
    """
    parsed, i = _parse_value(raw_json, 0)
    if _skip_ws(raw_json, i) != len(raw_json):
        raise ValueError(f"trailing data at {i}")
    return _render(parsed, excluded_top_level or None, True)


def canonicalize(value: object) -> str:
    """Canonicalize a Python value (numbers go through float semantics;
    u64-range integers must arrive as JSON text instead)."""
    return canonicalize_text(json.dumps(value, ensure_ascii=False, separators=(",", ":")))


def compute_event_hash(raw_event_json: str) -> str:
    """Recompute an event's content hash from its raw JSON text:
    SHA-256(canonical JSON minus top-level hash/digests/schema_version).

    Independent of Rust: only the documented canonicalization + digest rule.
    """
    canonical = canonicalize_text(raw_event_json, EVENT_HASH_EXCLUDED_KEYS)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def sha256_hex_of_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()
