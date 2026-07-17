//! Cursor tokenizer over the C++ `ConfigFile` byte format.
//!
//! Ports the read primitives from the C++ reference: separators from
//! `src/ConfigFile.h:9-12`, `readString`/`readVarString`/`readUInt*`/`readInt8`/
//! `readBool`/`readFloat` from `src/ConfigFile.cpp:103-292`, and `_rtrim` from
//! `src/Utils.h:42-45`. The C++ reads one byte at a time from a `File`; this
//! reader walks an in-memory `&[u8]` with identical field semantics.

use heapless::{String, Vec};

/// Field separator (`,`) — C++ `CFG_VALUE_SEP` (`ConfigFile.h:9`).
const VALUE_SEP: u8 = b',';
/// Record terminator (`\n`) — C++ `CFG_REC_END` (`ConfigFile.h:10`).
const REC_END: u8 = b'\n';
/// String quote token (`"`) — C++ `CFG_TOK_QUOTE` (`ConfigFile.h:12`).
const QUOTE: u8 = b'"';

/// Fraction digits kept when parsing a decimal into hundredths.
const CENTI_FRAC_DIGITS: usize = 2;

/// Errors surfaced while tokenizing a C++ backup file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateError {
    /// The cursor reached the end of the input while a field was expected.
    ///
    /// **Divergence from C++.** The reference `ConfigFile::readString` returns
    /// `false` on a read failure (`ConfigFile.cpp:122-123`) and the numeric
    /// readers substitute their default (`0`, e.g. `ConfigFile.cpp:253`). A
    /// migrator must instead detect a truncated or corrupt backup, so a read
    /// attempted with nothing left to consume is an error rather than a silent
    /// default. A field that is present but *empty* (terminated immediately by a
    /// separator, e.g. `,`) is still `Ok` and parses to `0`/`""`, matching C++.
    UnexpectedEof,
    /// A numeric field could not be parsed.
    ///
    /// Reserved: the C++ `atoi`/`atof` path never fails (it returns `0` on
    /// garbage), so the faithful parsers here do not currently produce this. It
    /// exists for the record-level parsers built on top of this tokenizer.
    BadNumber,
    /// A decoded string did not fit the caller's `heapless::String` capacity.
    ///
    /// **Divergence from C++.** The reference silently truncates at its fixed
    /// buffer length and `_rtrim`s (`ConfigFile.cpp:117-119`); a migrator errors
    /// so an over-long name is not quietly corrupted.
    StringTooLong,
    /// The backup declared a header version this migrator does not understand.
    ///
    /// Reserved for the header parser built on top of this tokenizer.
    UnsupportedVersion(u8),
    /// A record was structurally invalid; the payload names the offending part.
    BadRecord(&'static str),
}

/// Cursor over a C++ `ConfigFile` byte buffer.
///
/// Construct with [`Reader::new`], then pull fields in record order with the
/// `read_*` methods. Each successful read advances past the field's trailing
/// separator, so consecutive calls walk the comma-separated record left to
/// right; [`Reader::skip_record_end`] jumps to the start of the next record.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Wrap a byte slice for tokenizing, positioned at the first byte.
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    /// `true` once every byte has been consumed.
    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// Scan to the next `,`/`\n`, returning the field bytes and consuming the
    /// terminator.
    ///
    /// Mirrors C++ `readString` (`ConfigFile.cpp:103-127`): the separator is
    /// read and discarded, not returned. Leading/trailing padding is preserved
    /// here — callers apply `_rtrim`/`atoi`-style trimming as the C++ readers do.
    fn next_field(&mut self) -> Result<&'a [u8], MigrateError> {
        let data = self.data;
        if self.pos >= data.len() {
            return Err(MigrateError::UnexpectedEof);
        }
        let start = self.pos;
        let mut i = start;
        while i < data.len() {
            match data[i] {
                VALUE_SEP | REC_END => {
                    self.pos = i + 1; // consume the terminator (C++ :110-114)
                    return Ok(&data[start..i]);
                }
                _ => i += 1,
            }
        }
        // No terminator before end-of-input: the remaining bytes are the field.
        // C++ `readString` returns `false` here (a read failure); we keep the
        // trailing content rather than discarding it — see `UnexpectedEof`.
        self.pos = i;
        Ok(&data[start..i])
    }

    /// Read a fixed-width string field, right-trimming C++ padding.
    ///
    /// Reads until `,`/`\n` (consuming it) and strips trailing
    /// ` `/`\n`/`\r`/`\t`/`"` exactly like C++ `_rtrim` (`Utils.h:42-45`),
    /// mirroring `ConfigFile::readString` (`ConfigFile.cpp:103-127`). Fails with
    /// [`MigrateError::StringTooLong`] if the trimmed value exceeds `out`'s
    /// capacity and [`MigrateError::BadRecord`] on invalid UTF-8.
    pub fn read_str(&mut self, out: &mut String<64>) -> Result<(), MigrateError> {
        let field = rtrim(self.next_field()?);
        out.clear();
        let s = core::str::from_utf8(field).map_err(|_| MigrateError::BadRecord("utf8"))?;
        out.push_str(s).map_err(|_| MigrateError::StringTooLong)
    }

    /// Read a quote-aware string field (C++ `readVarString`,
    /// `ConfigFile.cpp:151-185`).
    ///
    /// A `"` toggles quote state and is never stored (`ConfigFile.cpp:170-172`).
    /// A separator ends the field only once the closing quote has been seen
    /// (`quotes >= 2`, `ConfigFile.cpp:162-166`); a separator seen earlier is
    /// treated as content, so a quoted value may contain commas.
    ///
    /// **Divergence from brief.** The brief described the unquoted path as
    /// "defer to `read_str`", but the C++ does not: with no opening quote,
    /// commas are absorbed as content and the field ends only at `\n`/EOF. This
    /// reproduces the C++ exactly. `_rtrim` is applied on the separator/EOF/full
    /// paths but *not* the `\n` path (`ConfigFile.cpp:168-169` returns without
    /// trimming), which is reproduced here as well.
    pub fn read_var_str(&mut self, out: &mut String<64>) -> Result<(), MigrateError> {
        out.clear();
        let data = self.data;
        if self.pos >= data.len() {
            return Err(MigrateError::UnexpectedEof);
        }
        let mut buf: Vec<u8, 64> = Vec::new();
        let mut quotes: u8 = 0;
        let mut ended_on_rec = false;
        while self.pos < data.len() {
            let b = data[self.pos];
            self.pos += 1;
            match b {
                VALUE_SEP if quotes >= 2 => break,
                REC_END => {
                    ended_on_rec = true;
                    break;
                }
                // saturating: only the `>= 2` threshold matters, so a field with
                // hundreds of quotes must not overflow the u8 counter in debug.
                QUOTE => quotes = quotes.saturating_add(1),
                _ => buf.push(b).map_err(|_| MigrateError::StringTooLong)?,
            }
        }
        let bytes = if ended_on_rec { &buf[..] } else { rtrim(&buf) };
        let s = core::str::from_utf8(bytes).map_err(|_| MigrateError::BadRecord("utf8"))?;
        out.push_str(s).map_err(|_| MigrateError::StringTooLong)
    }

    /// Read an unsigned 8-bit decimal field (C++ `readUInt8`,
    /// `ConfigFile.cpp:255-260`): `atoi` then `static_cast<uint8_t>`.
    pub fn read_u8(&mut self) -> Result<u8, MigrateError> {
        Ok(atoi(self.next_field()?) as u8)
    }

    /// Read an unsigned 16-bit decimal field (C++ `readUInt16`,
    /// `ConfigFile.cpp:261-266`).
    pub fn read_u16(&mut self) -> Result<u16, MigrateError> {
        Ok(atoi(self.next_field()?) as u16)
    }

    /// Read an unsigned 32-bit decimal field (C++ `readUInt32`,
    /// `ConfigFile.cpp:267-272`).
    ///
    /// The digits are accumulated into `i64` before the cast; this is wider than
    /// the C++ `int` used by `atoi`, so values above `INT_MAX` (which the C++
    /// reader mangles) round-trip correctly — a benign correctness improvement.
    pub fn read_u32(&mut self) -> Result<u32, MigrateError> {
        Ok(atoi(self.next_field()?) as u32)
    }

    /// Read a signed 8-bit decimal field (C++ `readInt8`,
    /// `ConfigFile.cpp:249-254`): `atoi` then `static_cast<int8_t>`.
    pub fn read_i8(&mut self) -> Result<i8, MigrateError> {
        Ok(atoi(self.next_field()?) as i8)
    }

    /// Read a boolean field.
    ///
    /// C++ `writeBool` emits `"true"`/`"false"` (`ConfigFile.cpp:242`) and
    /// `readBool` inspects only the first byte for `t`/`T`/`1`
    /// (`ConfigFile.cpp:282-288`); every other value is `false`.
    ///
    /// **Divergence from brief.** The brief stated bools are written as `1`/`0`;
    /// the C++ actually writes `true`/`false`, so this matches the first byte
    /// (which also accepts the `1`/`0` form the brief expected).
    pub fn read_bool(&mut self) -> Result<bool, MigrateError> {
        let field = self.next_field()?;
        Ok(matches!(field.first(), Some(b't' | b'T' | b'1')))
    }

    /// Parse a fixed-point decimal into signed hundredths (centi-units).
    ///
    /// The C++ writes positions with `writeFloat(pos, 5)` — `%12.5f`, e.g.
    /// `"    42.50000"` (`ConfigFile.cpp:236-239`, call sites `:995-998`). This
    /// skips leading whitespace and an optional sign, reads the integer part,
    /// then up to two fraction digits (right-padded to two; **further fraction
    /// digits are truncated**, so the 5-digit position output is tolerated), and
    /// applies the sign to the combined value. No float types are used.
    ///
    /// `"42.50000"` → `4250`, `"-1.00000"` → `-100`, `"7"` → `700`,
    /// `"0.5"` → `50`.
    ///
    /// Note: fields the C++ writes with a different precision (radio
    /// `frequency` at prec 3, `ConfigFile.cpp:1064`) lose sub-centi digits here;
    /// a milli-scale variant belongs to the record parser that consumes them.
    pub fn read_f32_as_centi(&mut self) -> Result<i32, MigrateError> {
        let field = self.next_field()?;
        let mut i = skip_ws(field, 0);
        let mut neg = false;
        if let Some(&b) = field.get(i) {
            if b == b'+' || b == b'-' {
                neg = b == b'-';
                i += 1;
            }
        }
        let mut centi: i64 = 0;
        while let Some(&b) = field.get(i) {
            if !b.is_ascii_digit() {
                break;
            }
            centi = centi.wrapping_mul(10).wrapping_add((b - b'0') as i64);
            i += 1;
        }
        centi = centi.wrapping_mul(100);
        if field.get(i) == Some(&b'.') {
            i += 1;
            let mut scale: i64 = 10;
            for _ in 0..CENTI_FRAC_DIGITS {
                match field.get(i) {
                    Some(&b) if b.is_ascii_digit() => {
                        centi = centi.wrapping_add((b - b'0') as i64 * scale);
                        scale /= 10;
                        i += 1;
                    }
                    _ => break,
                }
            }
            // Any remaining fraction digits are truncated (centi precision).
        }
        Ok((if neg { -centi } else { centi }) as i32)
    }

    /// Advance past the next record terminator (`\n`).
    ///
    /// Scans forward, consuming bytes up to and including the next `\n`; returns
    /// `Ok` at EOF without one. This always consumes a whole record's worth of
    /// bytes, so it is the right tool for skipping an *unparsed* trailing record
    /// (repeater/settings/net/trans). To defensively realign after parsing a
    /// record's fields, prefer [`Reader::resync_record`], which is a no-op when
    /// the cursor already sits on a record boundary.
    pub fn skip_record_end(&mut self) -> Result<(), MigrateError> {
        let data = self.data;
        while self.pos < data.len() {
            let b = data[self.pos];
            self.pos += 1;
            if b == REC_END {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `true` when the cursor sits at the start of a record — at the very
    /// beginning of the input, or immediately after a consumed record end
    /// (`\n`), or at EOF reached exactly on a terminator.
    ///
    /// This is the variable-width analogue of the C++ record-boundary check
    /// `file.position() == startPos + recordSize` (`ConfigFile.cpp:880`): since
    /// this reader tolerates non-fixed-width fields, alignment is expressed as
    /// "the last terminator consumed was a record end" rather than a byte count.
    pub fn at_record_boundary(&self) -> bool {
        self.pos == 0 || self.data[self.pos - 1] == REC_END
    }

    /// Realign to the next record boundary, but only if the cursor is mid-record.
    ///
    /// Faithful port of the defensive `seekChar(CFG_REC_END)` the C++ record
    /// readers run *only* when a record was not fully consumed
    /// (`ConfigFile.cpp:794-797`, `:880-883`, `:771-774`): if the last field
    /// read ended on a value separator — more trailing fields remain in the
    /// record — this advances past the record end; if the last read already
    /// consumed the record end this is a no-op, so it never eats into the
    /// following record (which an unconditional [`Reader::skip_record_end`]
    /// would).
    ///
    /// Returns the number of leftover **content** bytes skipped (the trailing
    /// record end is structure, not data, so it is excluded). A nonzero return
    /// means the record carried fields this parser did not consume — a real
    /// misalignment risk (e.g. an unescaped comma inside a name shifts every
    /// field), so [`parse_backup`](crate::parse_backup) surfaces the count on
    /// [`MigrationData::skipped_resyncs`](crate::MigrationData) rather than
    /// silently trusting the parse. A no-op (already aligned) or a bare empty
    /// trailing field returns `0`.
    pub fn resync_record(&mut self) -> Result<usize, MigrateError> {
        if self.at_record_boundary() {
            return Ok(0);
        }
        let start = self.pos;
        self.skip_record_end()?;
        // skip_record_end consumed the leftover field bytes plus the record end
        // (or ran to EOF). The trailing `\n`, when present, is a separator, not
        // misparsed data, so report only the leftover content bytes.
        let consumed = self.pos - start;
        let content = if self.pos > start && self.data[self.pos - 1] == REC_END {
            consumed - 1
        } else {
            consumed
        };
        Ok(content)
    }
}

/// Index of the first non-ASCII-whitespace byte at or after `from`.
fn skip_ws(field: &[u8], from: usize) -> usize {
    let mut i = from;
    while matches!(field.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// `true` for bytes stripped by C++ `_rtrim` (`Utils.h:44`): space, `\n`, `\r`,
/// `\t`, and `"`.
fn is_rtrim_byte(b: u8) -> bool {
    matches!(b, b' ' | b'\n' | b'\r' | b'\t' | b'"')
}

/// Right-trim a field the way C++ `_rtrim` does (`Utils.h:42-45`).
fn rtrim(field: &[u8]) -> &[u8] {
    let mut end = field.len();
    while end > 0 && is_rtrim_byte(field[end - 1]) {
        end -= 1;
    }
    &field[..end]
}

/// C `atoi` semantics: skip leading ASCII whitespace, take an optional sign,
/// accumulate digits, stop at the first non-digit; no digits ⇒ `0`
/// (C++ numeric readers hand the field to `atoi`, e.g. `ConfigFile.cpp:252`).
///
/// Accumulates into `i64` (wider than C++'s `int`) with wrapping arithmetic so
/// pathological digit runs cannot panic in debug builds; the callers cast the
/// result to the target width, reproducing the C++ `static_cast` truncation.
fn atoi(field: &[u8]) -> i64 {
    let mut i = skip_ws(field, 0);
    let mut neg = false;
    if let Some(&b) = field.get(i) {
        if b == b'+' || b == b'-' {
            neg = b == b'-';
            i += 1;
        }
    }
    let mut val: i64 = 0;
    while let Some(&b) = field.get(i) {
        if !b.is_ascii_digit() {
            break;
        }
        val = val.wrapping_mul(10).wrapping_add((b - b'0') as i64);
        i += 1;
    }
    if neg {
        -val
    } else {
        val
    }
}
