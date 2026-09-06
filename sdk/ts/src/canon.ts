// Independent TypeScript implementation of edda-canon-v1 canonical JSON.
//
// Canonical form (pinned by spec/events/canonical-v1.json and the Rust
// `canonical_byte_vectors_match_before_hashing` test):
//   - object keys sorted lexicographically by Unicode code point order
//     (== UTF-8 byte order), recursive; arrays keep order; compact separators.
//   - strings are decoded and re-emitted per serde_json escaping rules:
//     `"` and `\` escaped, C0 controls as \b \t \n \f \r or \u00xx (lowercase
//     hex), everything else (incl. all non-ASCII) emitted raw.
//   - numbers are parsed and re-emitted per serde_json (zmij) semantics:
//     * integer-form lexemes that fit u64/i64 are emitted as exact integers
//       (u64 max and i64 min survive); larger integer lexemes fall back to f64.
//     * everything else is an f64, emitted as shortest-roundtrip digits with
//       plain decimal notation when the decimal exponent is in [-5, 15]
//       (always with at least one fractional digit: 1.0, -0.0), otherwise
//       exponential with explicit sign and no zero padding: 1e+30, 1e-7.
//   * non-finite results (e.g. 1e999) are an error — serde_json refuses
//       them too.
//
// Hash rule (docs/reference/ledger-event-spec.md): event hash = SHA-256 of
// the canonical JSON of the event with top-level `hash`, `digests` and
// `schema_version` removed.
//
// Scope honesty: arbitrary-precision numbers beyond u64/i64 integer form are
// NOT preserved (they degrade to f64, matching serde_json); -0.0 integer
// lexeme ("-0") normalizes to "0" (integer path), while float "-0.0" stays
// "-0.0".

/** Compare two strings by Unicode code point order (== UTF-8 byte order,
 * matching Rust `str` cmp). Compares by code points, not UTF-16 units. */
export function compareCodePoints(a: string, b: string): number {
  const enc = new TextEncoder();
  const ba = enc.encode(a);
  const bb = enc.encode(b);
  const n = Math.min(ba.length, bb.length);
  for (let i = 0; i < n; i++) {
    if (ba[i] !== bb[i]) return ba[i] - bb[i];
  }
  return ba.length - bb.length;
}

const WHITESPACE = new Set([" ", "\t", "\n", "\r"]);

function skipWs(s: string, i: number): number {
  while (i < s.length && WHITESPACE.has(s[i])) i++;
  return i;
}

/** Parse a JSON string token starting at s[i] === '"'; returns the DECODED string. */
function parseStringDecoded(s: string, i: number): { value: string; next: number } {
  if (s[i] !== '"') throw new Error(`expected string at ${i}`);
  let out = "";
  let j = i + 1;
  for (;;) {
    if (j >= s.length) throw new Error(`unterminated string at ${i}`);
    const c = s[j];
    if (c === '"') return { value: out, next: j + 1 };
    if (c === "\\") {
      const e = s[j + 1];
      switch (e) {
        case '"': out += '"'; j += 2; break;
        case "\\": out += "\\"; j += 2; break;
        case "/": out += "/"; j += 2; break;
        case "b": out += "\b"; j += 2; break;
        case "f": out += "\f"; j += 2; break;
        case "n": out += "\n"; j += 2; break;
        case "r": out += "\r"; j += 2; break;
        case "t": out += "\t"; j += 2; break;
        case "u": {
          const hex = s.slice(j + 2, j + 6);
          if (!/^[0-9a-fA-F]{4}$/.test(hex)) throw new Error(`bad \\u escape at ${j}`);
          const cp = parseInt(hex, 16);
          if (cp >= 0xd800 && cp <= 0xdbff) {
            // high surrogate: require a matching \uDC00-\uDFFF
            if (s[j + 6] !== "\\" || s[j + 7] !== "u") {
              throw new Error(`lone high surrogate at ${j}`);
            }
            const lo = parseInt(s.slice(j + 8, j + 12), 16);
            if (!(lo >= 0xdc00 && lo <= 0xdfff)) throw new Error(`lone high surrogate at ${j}`);
            out += String.fromCharCode(cp, lo);
            j += 12;
          } else if (cp >= 0xdc00 && cp <= 0xdfff) {
            throw new Error(`lone low surrogate at ${j}`);
          } else {
            out += String.fromCharCode(cp);
            j += 6;
          }
          break;
        }
        default:
          throw new Error(`bad escape at ${j}`);
      }
    } else if (c < " ") {
      throw new Error(`raw control char in string at ${j}`);
    } else {
      out += c;
      j++;
    }
  }
}

/** Re-emit a decoded string per serde_json escaping rules. */
function emitString(decoded: string): string {
  let out = '"';
  for (const ch of decoded) {
    const c = ch.codePointAt(0)!;
    if (ch === '"') out += '\\"';
    else if (ch === "\\") out += "\\\\";
    else if (ch === "\b") out += "\\b";
    else if (ch === "\t") out += "\\t";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\f") out += "\\f";
    else if (ch === "\r") out += "\\r";
    else if (c < 0x20) out += "\\u" + c.toString(16).padStart(4, "0");
    else out += ch;
  }
  return out + '"';
}

const JSON_NUMBER = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;
const INTEGER_LEXEME = /^-?(0|[1-9][0-9]*)$/;
const I64_MIN = -(2n ** 63n);
const U64_MAX = 2n ** 64n - 1n;

/** Format an f64 per serde_json's (zmij) shortest-roundtrip emitter. */
function formatF64(f: number): string {
  if (!Number.isFinite(f)) throw new Error("number out of range (non-finite)");
  if (Object.is(f, -0)) return "-0.0";
  if (f === 0) return "0.0";
  // shortest digits + decimal exponent via toExponential (shortest by spec)
  const [mantissa, e] = f.toExponential().split("e");
  const sign = mantissa.startsWith("-") ? "-" : "";
  let digits = mantissa.replace("-", "").replace(".", "").replace(/0+$/, "");
  if (digits === "") digits = "0";
  const decExp = parseInt(e, 10);
  return sign + zmijFormat(digits, decExp);
}

/** zmij decimal/exponential layout from shortest digits. */
function zmijFormat(digits: string, decExp: number): string {
  const n = digits.length;
  if (decExp >= -5 && decExp <= 15) {
    if (n - 1 <= decExp) {
      // 1234e7 -> 12340000000.0 ; 1.0
      return digits + "0".repeat(decExp - (n - 1)) + ".0";
    } else if (decExp >= 0) {
      // 1234e-2 -> 12.34
      return digits.slice(0, decExp + 1) + "." + digits.slice(decExp + 1);
    } else {
      // 1234e-6 -> 0.001234
      return "0." + "0".repeat(-decExp - 1) + digits;
    }
  }
  // exponential: 1e+30, 1.234e+33, 1e-7
  const mant = n > 1 ? digits[0] + "." + digits.slice(1) : digits;
  return `${mant}e${decExp >= 0 ? "+" : "-"}${Math.abs(decExp)}`;
}

/** Canonical number emission from a raw lexeme (serde_json parse semantics). */
export function canonicalNumber(lexeme: string): string {
  if (!JSON_NUMBER.test(lexeme)) throw new Error(`invalid JSON number: ${lexeme}`);
  if (INTEGER_LEXEME.test(lexeme)) {
    const v = BigInt(lexeme);
    if (v >= I64_MIN && v <= U64_MAX) return v.toString();
    // serde_json falls back to f64 for integer lexemes beyond u64/i64
  }
  return formatF64(Number(lexeme));
}

interface JsonNode {
  kind: "object" | "array" | "scalar";
  entries?: Array<[string, string]>; // decoded key -> canonical text
  items?: string[];
  raw?: string; // pre-rendered canonical scalar
}

function parseValue(s: string, i: number): { value: JsonNode; next: number } {
  i = skipWs(s, i);
  const c = s[i];
  if (c === "{") {
    const map = new Map<string, string>();
    i = skipWs(s, i + 1);
    if (s[i] === "}") return { value: { kind: "object", entries: [] }, next: i + 1 };
    for (;;) {
      i = skipWs(s, i);
      if (s[i] !== '"') throw new Error(`expected object key at ${i}`);
      const key = parseStringDecoded(s, i);
      i = skipWs(s, key.next);
      if (s[i] !== ":") throw new Error(`expected ':' at ${i}`);
      const val = parseValue(s, i + 1);
      map.set(key.value, render(val.value)); // last key wins, like serde_json
      i = skipWs(s, val.next);
      if (s[i] === ",") {
        i++;
        continue;
      }
      if (s[i] === "}") return { value: { kind: "object", entries: [...map] }, next: i + 1 };
      throw new Error(`expected ',' or '}' at ${i}`);
    }
  }
  if (c === "[") {
    const items: string[] = [];
    i = skipWs(s, i + 1);
    if (s[i] === "]") return { value: { kind: "array", items }, next: i + 1 };
    for (;;) {
      const val = parseValue(s, i);
      items.push(render(val.value));
      i = skipWs(s, val.next);
      if (s[i] === ",") {
        i++;
        continue;
      }
      if (s[i] === "]") return { value: { kind: "array", items }, next: i + 1 };
      throw new Error(`expected ',' or ']' at ${i}`);
    }
  }
  if (c === '"') {
    const str = parseStringDecoded(s, i);
    return { value: { kind: "scalar", raw: emitString(str.value) }, next: str.next };
  }
  // number / true / false / null lexeme
  let j = i;
  while (j < s.length && !WHITESPACE.has(s[j]) && s[j] !== "," && s[j] !== "}" && s[j] !== "]") j++;
  if (j === i) throw new Error(`unexpected char at ${i}`);
  const lexeme = s.slice(i, j);
  let raw: string;
  if (lexeme === "true" || lexeme === "false" || lexeme === "null") raw = lexeme;
  else raw = canonicalNumber(lexeme);
  return { value: { kind: "scalar", raw }, next: j };
}

function render(v: JsonNode, excludedTopLevel?: ReadonlySet<string>, isTop = true): string {
  if (v.kind === "object" && v.entries) {
    const sorted = [...v.entries].sort((a, b) => compareCodePoints(a[0], b[0]));
    const parts: string[] = [];
    for (const [k, canonicalVal] of sorted) {
      // Exclusions apply only at depth 0 (event hash rule).
      if (isTop && excludedTopLevel && excludedTopLevel.has(k)) continue;
      parts.push(`${emitString(k)}:${canonicalVal}`);
    }
    return `{${parts.join(",")}}`;
  }
  if (v.kind === "array" && v.items) return `[${v.items.join(",")}]`;
  return v.raw ?? "";
}

/**
 * Canonicalize raw JSON text. Top-level keys named in `excludedTopLevel`
 * are dropped (event hash rule). Number and string emission follow
 * serde_json (zmij) semantics per spec/events/canonical-v1.json.
 */
export function canonicalizeText(
  rawJson: string,
  excludedTopLevel: ReadonlySet<string> = new Set(),
): string {
  const parsed = parseValue(rawJson, 0);
  if (skipWs(rawJson, parsed.next) !== rawJson.length) {
    throw new Error(`trailing data at ${parsed.next}`);
  }
  if (excludedTopLevel.size === 0) return render(parsed.value);
  return render(parsed.value, excludedTopLevel, true);
}

/** Canonicalize a JS value (numbers go through JS f64 semantics; u64-range
 * integers must be passed as strings/BigInt-bearing JSON text instead). */
export function canonicalize(value: unknown): string {
  return canonicalizeText(JSON.stringify(value));
}

const enc = new TextEncoder();

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await globalThis.crypto.subtle.digest("SHA-256", bytes as unknown as ArrayBuffer);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/** Keys removed before hashing an event, per the canonical hash rule. */
export const EVENT_HASH_EXCLUDED_KEYS: ReadonlySet<string> = new Set([
  "hash",
  "digests",
  "schema_version",
]);

/**
 * Recompute an event's content hash from its raw JSON text:
 * SHA-256(canonical JSON minus top-level hash/digests/schema_version).
 * Independent of Rust: only the documented canonicalization + digest rule.
 */
export async function computeEventHash(rawEventJson: string): Promise<string> {
  const canonical = canonicalizeText(rawEventJson, EVENT_HASH_EXCLUDED_KEYS);
  return sha256Hex(enc.encode(canonical));
}

/** SHA-256 of arbitrary text, hex-encoded. */
export async function sha256HexOfText(text: string): Promise<string> {
  return sha256Hex(enc.encode(text));
}
