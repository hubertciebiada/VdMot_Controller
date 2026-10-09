//! JSON request bodies with the behaviour of ArduinoJson 6.21.6, as the C++ glue parsed them with
//! `deserializeJson(StaticJsonDocument<512>&, char* body, size_t len)` on the ESP32
//! (docs/rust/GLUE-DESIGN-ESP.md 3.1, 4.4, decision 7.10). The glue answers a malformed body with
//! ArduinoJson's error name, so every leniency, limit and quirk of the library is kept.
//!
//! Parity: the bodies of `tests/json_body/corpus.txt` run through `tests/json_body/reference.cpp`
//! (`ArduinoJson-v6.21.6.h` as vendored in the C++ 2.1.7 tree, extracted by
//! `tools/rust/cpp217.sh`, with the firmware's switches, built with `-m32` for the ESP32's 16-byte
//! slots) and through this module; the outcomes of both are `tests/json_body/golden.txt`
//! (`json_body/tests_golden.rs`).
//!
//! What the deserializer (`JsonDeserializer` with `StringMover`, no filter) accepts:
//!
//! - Values: objects, arrays, strings in double or single quotes, numbers, `true`, `false`,
//!   `null`. Whitespace is space, TAB, CR and LF. Comments, `NaN` and `Infinity` are refused
//!   (`ARDUINOJSON_ENABLE_COMMENTS`, `_NAN` and `_INFINITY` are 0 in the firmware).
//! - Object keys: quoted like strings, or unquoted: one or more of `0-9`, `A-Z`, `a-z`, `_` and
//!   the backtick. A trailing comma is `InvalidInput` (`{"a":1,}`, `[1,]`).
//! - A NUL byte reads as the end of the input. After a root object, array, string, `true`,
//!   `false` or `null` the rest of the input is not read; a root number must be followed by the
//!   end, else `InvalidInput` (`7` is a number, `7 ` is `InvalidInput`).
//! - Strings: every byte except the closing quote, the backslash and NUL is taken as it is
//!   (control characters and bytes >= 0x80 too, no UTF-8 check). Escapes: `\" \\ \/ \b \f \n \r
//!   \t` and `\uXXXX`; `\'` and every other escape are `InvalidInput`. A hex digit below `A` is
//!   the character minus `0`, so `:` .. `?` count as 10..15; above, bit 5 is cleared before `A`
//!   is subtracted, so the backtick counts as 9. `\uXXXX` is written as UTF-8; a high
//!   surrogate (D800..DBFF) waits for a low one (DC00..DFFF) later in the same string and is
//!   dropped without one; a low surrogate pairs with the last high one of the string, or with 0
//!   (a lone `\uDC00` is U+10000). `\u0000` puts a NUL into the string, which ends its C string.
//! - Numbers: the characters `0-9 + - . e E`, at most 63 of them. An integer that fits u64 is
//!   stored unsigned, a negative one down to -2^63 signed, both with leading zeros and a leading
//!   `+` allowed. Every other number is a double from the library's own algorithm: a decimal
//!   mantissa of at most 52 bits (further digits are dropped, not rounded) times powers of ten
//!   by binary exponentiation, so a result can differ from a correctly rounded parse. A bare `.`
//!   is 0.0 and `1e` is 1.0; an exponent above 308 (mantissa shift included) gives ±0.0 or ±inf
//!   while it is read, so `0e400` is inf. An integer that overflows u64 at its last digit after
//!   the multiplication by ten comes out ten times too large (`18446744073709551616` reads as
//!   1.844674407370955e20), a library bug that is kept.
//! - Strings and keys are unescaped in place into the input (zero-copy), each followed by a NUL:
//!   the input is modified and the document refers to it, so the views borrow both.
//! - Memory: [`SLOT_COUNT`] slots for all object members and array elements of the tree, the
//!   next one is `NoMemory`. An array element takes its slot before it is parsed, a member after
//!   its key and colon. A key equal to an earlier key of the same object as a C string (up to its
//!   first NUL) reuses that member: the new value replaces the old one in place, except `null`,
//!   which leaves the old value; the slots of a replaced object or array stay used.
//! - Nesting: [`NESTING_LIMIT`] levels of objects and arrays, a deeper one is `TooDeep`.
//! - The error is the first one met in reading order.

/// Slots of the document: `StaticJsonDocument<512>` holds 32 `VariantSlot`s of 16 bytes on the
/// 32-bit ESP32 (the C++ host tests, with 32-byte slots, had 16).
pub const SLOT_COUNT: usize = 32;
/// Bytes per slot in [`JsonDocument::memory_usage`] (`sizeof(VariantSlot)` on the ESP32).
pub const SLOT_SIZE: usize = 16;
/// Levels of nested objects and arrays (`ARDUINOJSON_DEFAULT_NESTING_LIMIT`).
pub const NESTING_LIMIT: u8 = 10;

/// Longest input that is read: offsets are 32-bit as on the ESP32, whose `size_t` cannot
/// exceed it; bytes beyond it are ignored (only possible on a 64-bit host).
const MAX_INPUT: usize = u32::MAX as usize;
/// Characters of a number that are read (`JsonDeserializer::buffer_` minus the terminator).
const NUMBER_CHARS: usize = 63;
/// No slot: beyond the pool, so every lookup of it misses.
const NONE: u8 = u8::MAX;
/// Content of an object or array without slots: first and last slot [`NONE`].
const EMPTY_COLLECTION: u64 = 0xFFFF;
/// Largest mantissa of a double (`FloatTraits<double>::mantissa_max`).
const MANTISSA_MAX: u64 = (1 << 52) - 1;
/// Largest decimal exponent (`FloatTraits<double>::exponent_max`).
const EXPONENT_MAX: i32 = 308;
/// `FloatTraits<double>::nan()`.
const NAN_BITS: u64 = 0x7FF8_0000_0000_0000;
/// 1e1, 1e2, 1e4 .. 1e256 (`FloatTraits<double>::positiveBinaryPowersOfTen`).
const POSITIVE_POWERS: [u64; 9] = [
    0x4024_0000_0000_0000,
    0x4059_0000_0000_0000,
    0x40C3_8800_0000_0000,
    0x4197_D784_0000_0000,
    0x4341_C379_37E0_8000,
    0x4693_B8B5_B505_6E17,
    0x4D38_4F03_E93F_F9F5,
    0x5A82_7748_F930_1D32,
    0x7515_4FDD_7F73_BF3C,
];
/// 1e-1, 1e-2, 1e-4 .. 1e-256 (`FloatTraits<double>::negativeBinaryPowersOfTen`).
const NEGATIVE_POWERS: [u64; 9] = [
    0x3FB9_9999_9999_999A,
    0x3F84_7AE1_47AE_147B,
    0x3F1A_36E2_EB1C_432D,
    0x3E45_798E_E230_8C3A,
    0x3C9C_D2B2_97D8_89BC,
    0x3949_F623_D5A8_A733,
    0x32A5_0FFD_44F4_A73D,
    0x255B_BA08_CF8C_979D,
    0x0AC8_0628_64AC_6F43,
];
/// `numeric_limits<long long>::lowest()` as a double: -2^63.
const I64_LOWEST: f64 = -9_223_372_036_854_775_808.0;
/// `FloatTraits<double>::highest_for<long long>()`: the largest double below 2^63.
const I64_HIGHEST: u64 = 0x43DF_FFFF_FFFF_FFFF;

/// Why a body was refused (`DeserializationError::Code` without `Ok`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DeserializationError {
    /// Nothing but whitespace before the end of the input (or a NUL byte).
    EmptyInput = 1,
    /// The input (or a NUL byte) ends inside a value.
    IncompleteInput = 2,
    /// A character the grammar does not allow there, or a root number followed by more input.
    InvalidInput = 3,
    /// A member or element beyond the [`SLOT_COUNT`] slots.
    NoMemory = 4,
    /// An object or array deeper than [`NESTING_LIMIT`] levels.
    TooDeep = 5,
}

impl DeserializationError {
    /// ArduinoJson's name of the error (`c_str()`), the detail of the glue's 400 answer.
    pub const fn c_str(self) -> &'static str {
        match self {
            Self::EmptyInput => "EmptyInput",
            Self::IncompleteInput => "IncompleteInput",
            Self::InvalidInput => "InvalidInput",
            Self::NoMemory => "NoMemory",
            Self::TooDeep => "TooDeep",
        }
    }
}

/// Type of a slot (`VariantData::type()`; strings are always linked, zero-copy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Null,
    Bool,
    Unsigned,
    Signed,
    Float,
    Str,
    Object,
    Array,
}

/// A value in the pool (`VariantSlot`) or the root (`VariantData`): 16 bytes as on the ESP32.
#[derive(Clone, Copy, Debug)]
struct Slot {
    /// Bool: 0 or 1; Unsigned: the value; Signed: two's complement; Float: the bits; Str: offset
    /// in the input (bits 0..32) and length (bits 32..64); Object, Array: first slot (bits 0..8)
    /// and last slot (bits 8..16), [`NONE`] when empty.
    content: u64,
    /// Members: offset of the key in the input (a C string).
    key: u32,
    kind: Kind,
    /// Next slot of the same object or array, [`NONE`] after the last one.
    next: u8,
}

const _: () = assert!(core::mem::size_of::<Slot>() == SLOT_SIZE);

impl Slot {
    /// A cleared slot (`VariantSlot::clear()`); its content reads as an empty collection.
    const NULL: Self = Self {
        content: EMPTY_COLLECTION,
        key: 0,
        kind: Kind::Null,
        next: NONE,
    };

    /// First slot of an object or array.
    fn first(&self) -> u8 {
        self.content as u8
    }

    /// Last slot of an object or array.
    fn last(&self) -> u8 {
        (self.content >> 8) as u8
    }
}

/// A number as `parseNumber` stores it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Number {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
}

impl Number {
    /// `asIntegral<long long>()`: unsigned above i64::MAX and doubles outside
    /// [-2^63, 2^63) (NaN too) are 0, other doubles are truncated.
    fn to_i64(self) -> i64 {
        match self {
            Number::Unsigned(u) => i64::try_from(u).unwrap_or(0),
            Number::Signed(s) => s,
            Number::Float(f) => {
                if (I64_LOWEST..=f64::from_bits(I64_HIGHEST)).contains(&f) {
                    f as i64
                } else {
                    0
                }
            }
        }
    }

    /// `asFloat<double>()`: integers rounded to the nearest double.
    fn to_f64(self) -> f64 {
        match self {
            Number::Unsigned(u) => u as f64,
            Number::Signed(s) => s as f64,
            Number::Float(f) => f,
        }
    }
}

/// A value decoded from its slot.
enum Value<'a> {
    Null,
    Bool(bool),
    Number(Number),
    /// The stored string (embedded NULs included).
    Str(&'a [u8]),
    /// First slot of the object.
    Object(u8),
    /// First slot of the array.
    Array(u8),
}

/// The document (`StaticJsonDocument<512>`): the root and a pool of [`SLOT_COUNT`] slots, 536
/// bytes without heap. It lives in the web working set and is reused for every body.
#[derive(Debug)]
pub struct JsonDocument {
    slots: [Slot; SLOT_COUNT],
    root: Slot,
    /// Slots taken from the pool since the last [`JsonDocument::deserialize`].
    used: u8,
}

/// Where the parser stores a value.
#[derive(Clone, Copy)]
enum At {
    Root,
    Slot(u8),
}

impl JsonDocument {
    /// An empty document: null root, no slot used (`push(JsonDocument::EMPTY)` into a box).
    pub const EMPTY: Self = Self {
        slots: [Slot::NULL; SLOT_COUNT],
        root: Slot::NULL,
        used: 0,
    };

    /// `deserializeJson(doc, input, input.len())` with `input` as the mutable `char*` body:
    /// clears the document, parses `input` (see the module documentation) and returns the root
    /// value. Strings are unescaped in place into `input`, so the root view borrows both.
    ///
    /// On an error the document holds what was parsed up to it ([`Self::memory_usage`]).
    pub fn deserialize<'a>(
        &'a mut self,
        input: &'a mut [u8],
    ) -> Result<JsonVariantConst<'a>, DeserializationError> {
        self.root = Slot::NULL;
        self.used = 0;
        let end = input.len().min(MAX_INPUT);
        let mut parser = Parser {
            doc: self,
            input: input.get_mut(..end).unwrap_or_default(),
            read: 0,
            current: 0,
            loaded: false,
            write: 0,
            start: 0,
            found_something: false,
            number: [0; NUMBER_CHARS],
        };
        parser.parse()?;
        let Parser { doc, input, .. } = parser;
        let doc: &'a JsonDocument = doc;
        Ok(JsonVariantConst {
            doc,
            input,
            data: Some(&doc.root),
        })
    }

    /// Bytes of the pool in use (`memoryUsage()`): [`SLOT_SIZE`] per slot taken since the last
    /// [`Self::deserialize`], the slots left behind by a replaced duplicate member included.
    pub fn memory_usage(&self) -> usize {
        usize::from(self.used) * SLOT_SIZE
    }

    fn data(&self, at: At) -> Option<&Slot> {
        match at {
            At::Root => Some(&self.root),
            At::Slot(i) => self.slots.get(usize::from(i)),
        }
    }

    fn data_mut(&mut self, at: At) -> Option<&mut Slot> {
        match at {
            At::Root => Some(&mut self.root),
            At::Slot(i) => self.slots.get_mut(usize::from(i)),
        }
    }

    /// `setType` and the content of a value (`toObject()`, `setBoolean()`, ...).
    fn set(&mut self, at: At, kind: Kind, content: u64) {
        if let Some(data) = self.data_mut(at) {
            data.kind = kind;
            data.content = content;
        }
    }

    fn set_number(&mut self, at: At, number: Number) {
        match number {
            Number::Unsigned(u) => self.set(at, Kind::Unsigned, u),
            Number::Signed(s) => self.set(at, Kind::Signed, s as u64),
            Number::Float(f) => self.set(at, Kind::Float, f.to_bits()),
        }
    }

    /// `CollectionData::addSlot`: a cleared slot at the end of the object or array at `owner`;
    /// `None` when the pool is used up.
    fn add_slot(&mut self, owner: At) -> Option<u8> {
        let index = self.used;
        *self.slots.get_mut(usize::from(index))? = Slot::NULL;
        self.used += 1;
        let (first, last) = self
            .data(owner)
            .map_or((NONE, NONE), |d| (d.first(), d.last()));
        let first = match self.slots.get_mut(usize::from(last)) {
            Some(tail) => {
                tail.next = index;
                first
            }
            None => index,
        };
        if let Some(data) = self.data_mut(owner) {
            data.content = u64::from(first) + (u64::from(index) << 8);
        }
        Some(index)
    }

    /// `CollectionData::getMember` for the key at `key` in `input`: the first member of the
    /// object at `owner` whose key equals it as a C string.
    fn find_member(&self, owner: At, input: &[u8], key: usize) -> Option<u8> {
        let first = self.data(owner).map_or(NONE, Slot::first);
        let key = cstr_at(input, key);
        Slots::new(self, input, first)
            .find(|(_, slot)| cstr_at(input, slot.key as usize) == key)
            .map(|(index, _)| index)
    }
}

/// A value of the document, as ArduinoJson's `JsonVariantConst`: the root, a member, an
/// element, or nothing (unbound: the lookup of a missing key), with the library's queries.
#[derive(Clone, Copy, Debug)]
pub struct JsonVariantConst<'a> {
    doc: &'a JsonDocument,
    input: &'a [u8],
    data: Option<&'a Slot>,
}

impl<'a> JsonVariantConst<'a> {
    fn value(&self) -> Value<'a> {
        let Some(slot) = self.data else {
            return Value::Null;
        };
        match slot.kind {
            Kind::Null => Value::Null,
            Kind::Bool => Value::Bool(slot.content != 0),
            Kind::Unsigned => Value::Number(Number::Unsigned(slot.content)),
            Kind::Signed => Value::Number(Number::Signed(slot.content as i64)),
            Kind::Float => Value::Number(Number::Float(f64::from_bits(slot.content))),
            Kind::Str => {
                let start = slot.content as u32 as usize;
                let end = start + (slot.content >> 32) as usize;
                Value::Str(self.input.get(start..end).unwrap_or_default())
            }
            Kind::Object => Value::Object(slot.first()),
            Kind::Array => Value::Array(slot.first()),
        }
    }

    /// `isNull()`: unbound or `null`.
    pub fn is_null(&self) -> bool {
        matches!(self.value(), Value::Null)
    }

    /// `is<long long>()`: an integer that fits i64 (a double never is).
    pub fn is_i64(&self) -> bool {
        match self.value() {
            Value::Number(Number::Unsigned(u)) => i64::try_from(u).is_ok(),
            Value::Number(Number::Signed(_)) => true,
            _ => false,
        }
    }

    /// `as<long long>()`: integers that fit i64 as they are, doubles in [-2^63, 2^63) truncated,
    /// `true` 1, a string parsed as a number (its C string, every length), anything else 0.
    pub fn as_i64(&self) -> i64 {
        match self.value() {
            Value::Bool(b) => i64::from(b),
            Value::Number(n) => n.to_i64(),
            Value::Str(s) => parse_number(cstr(s)).map_or(0, Number::to_i64),
            _ => 0,
        }
    }

    /// `is<double>()`: any number, integers included.
    pub fn is_f64(&self) -> bool {
        matches!(self.value(), Value::Number(_))
    }

    /// `as<double>()`: numbers (integers rounded to the nearest double), `true` 1.0, a string
    /// parsed as a number (its C string), anything else 0.0.
    pub fn as_f64(&self) -> f64 {
        match self.value() {
            Value::Bool(b) => f64::from(u8::from(b)),
            Value::Number(n) => n.to_f64(),
            Value::Str(s) => parse_number(cstr(s)).map_or(0.0, Number::to_f64),
            _ => 0.0,
        }
    }

    /// `is<bool>()`: `true` or `false`.
    pub fn is_bool(&self) -> bool {
        matches!(self.value(), Value::Bool(_))
    }

    /// `as<bool>()`: booleans as they are, numbers other than 0 (NaN too), every string, object
    /// and array are true; null and unbound false.
    pub fn as_bool(&self) -> bool {
        match self.value() {
            Value::Null => false,
            Value::Bool(b) => b,
            Value::Number(Number::Unsigned(u)) => u != 0,
            Value::Number(Number::Signed(s)) => s != 0,
            Value::Number(Number::Float(f)) => f != 0.0,
            Value::Str(_) | Value::Object(_) | Value::Array(_) => true,
        }
    }

    /// `is<const char*>()`: a string.
    pub fn is_str(&self) -> bool {
        matches!(self.value(), Value::Str(_))
    }

    /// `as<const char*>()`: the string as the glue used it with `strcmp`, up to its first NUL
    /// (`\u0000` cuts it); `None` (nullptr) for any other value.
    pub fn as_str(&self) -> Option<&'a [u8]> {
        match self.value() {
            Value::Str(s) => Some(cstr(s)),
            _ => None,
        }
    }

    /// `is<JsonObjectConst>()`: an object.
    pub fn is_object(&self) -> bool {
        matches!(self.value(), Value::Object(_))
    }

    /// `as<JsonObjectConst>()`: the object, `None` (a null object) for any other value.
    pub fn as_object(&self) -> Option<JsonObjectConst<'a>> {
        match self.value() {
            Value::Object(first) => Some(JsonObjectConst {
                slots: self.slots(first),
            }),
            _ => None,
        }
    }

    /// `is<JsonArrayConst>()`: an array.
    pub fn is_array(&self) -> bool {
        matches!(self.value(), Value::Array(_))
    }

    /// `as<JsonArrayConst>()`: the array, `None` (a null array) for any other value.
    pub fn as_array(&self) -> Option<JsonArrayConst<'a>> {
        match self.value() {
            Value::Array(first) => Some(JsonArrayConst {
                slots: self.slots(first),
            }),
            _ => None,
        }
    }

    /// `size()`: members of an object, elements of an array, 0 for any other value.
    pub fn size(&self) -> usize {
        match self.value() {
            Value::Object(first) | Value::Array(first) => self.slots(first).count(),
            _ => 0,
        }
    }

    fn slots(&self, first: u8) -> Slots<'a> {
        Slots::new(self.doc, self.input, first)
    }
}

/// An object of the document, as ArduinoJson's `JsonObjectConst`.
#[derive(Clone, Copy, Debug)]
pub struct JsonObjectConst<'a> {
    slots: Slots<'a>,
}

impl<'a> JsonObjectConst<'a> {
    /// `size()`: the members (a duplicate key counts once).
    pub fn size(&self) -> usize {
        self.slots.count()
    }

    /// `o[key]`: the first member whose key equals `key` as C strings (both up to their first
    /// NUL, case-sensitive); unbound (null) when there is none.
    pub fn get(&self, key: &[u8]) -> JsonVariantConst<'a> {
        let key = cstr(key);
        let (doc, input) = (self.slots.doc, self.slots.input);
        let data = { self.slots }
            .find(|(_, slot)| cstr_at(input, slot.key as usize) == key)
            .map(|(_, slot)| slot);
        JsonVariantConst { doc, input, data }
    }

    /// The members in document order (`for (JsonPairConst kv : o)`): the key as a C string
    /// (`kv.key().c_str()`) and the value.
    pub fn iter(&self) -> Members<'a> {
        Members { slots: self.slots }
    }
}

impl<'a> IntoIterator for JsonObjectConst<'a> {
    type Item = (&'a [u8], JsonVariantConst<'a>);
    type IntoIter = Members<'a>;

    fn into_iter(self) -> Members<'a> {
        self.iter()
    }
}

/// An array of the document, as ArduinoJson's `JsonArrayConst`.
#[derive(Clone, Copy, Debug)]
pub struct JsonArrayConst<'a> {
    slots: Slots<'a>,
}

impl<'a> JsonArrayConst<'a> {
    /// `size()`: the elements.
    pub fn size(&self) -> usize {
        self.slots.count()
    }

    /// The elements in document order.
    pub fn iter(&self) -> Elements<'a> {
        Elements { slots: self.slots }
    }
}

impl<'a> IntoIterator for JsonArrayConst<'a> {
    type Item = JsonVariantConst<'a>;
    type IntoIter = Elements<'a>;

    fn into_iter(self) -> Elements<'a> {
        self.iter()
    }
}

/// The slots of an object or array from `next` on, with their indices.
#[derive(Clone, Copy, Debug)]
struct Slots<'a> {
    doc: &'a JsonDocument,
    input: &'a [u8],
    next: u8,
    /// Steps left: no list is longer than the pool, so not even a broken one could loop.
    left: u8,
}

impl<'a> Slots<'a> {
    fn new(doc: &'a JsonDocument, input: &'a [u8], first: u8) -> Self {
        Self {
            doc,
            input,
            next: first,
            left: SLOT_COUNT as u8,
        }
    }
}

impl<'a> Iterator for Slots<'a> {
    type Item = (u8, &'a Slot);

    fn next(&mut self) -> Option<(u8, &'a Slot)> {
        self.left = self.left.checked_sub(1)?;
        let index = self.next;
        let slot = self.doc.slots.get(usize::from(index))?;
        self.next = slot.next;
        Some((index, slot))
    }
}

/// Iterator over the members of a [`JsonObjectConst`]: key (C string) and value.
#[derive(Clone, Debug)]
pub struct Members<'a> {
    slots: Slots<'a>,
}

impl<'a> Iterator for Members<'a> {
    type Item = (&'a [u8], JsonVariantConst<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let (doc, input) = (self.slots.doc, self.slots.input);
        let (_, slot) = self.slots.next()?;
        Some((
            cstr_at(input, slot.key as usize),
            JsonVariantConst {
                doc,
                input,
                data: Some(slot),
            },
        ))
    }
}

/// Iterator over the elements of a [`JsonArrayConst`].
#[derive(Clone, Debug)]
pub struct Elements<'a> {
    slots: Slots<'a>,
}

impl<'a> Iterator for Elements<'a> {
    type Item = JsonVariantConst<'a>;

    fn next(&mut self) -> Option<JsonVariantConst<'a>> {
        let (doc, input) = (self.slots.doc, self.slots.input);
        let (_, slot) = self.slots.next()?;
        Some(JsonVariantConst {
            doc,
            input,
            data: Some(slot),
        })
    }
}

/// The C string at the start of `bytes`: up to the first NUL.
fn cstr(bytes: &[u8]) -> &[u8] {
    bytes.split(|&b| b == 0).next().unwrap_or(bytes)
}

/// The C string at `offset` of `input`.
fn cstr_at(input: &[u8], offset: usize) -> &[u8] {
    cstr(input.get(offset..).unwrap_or_default())
}

/// `JsonDeserializer<BoundedReader<char*>, StringMover>` with `AllowAllFilter`.
struct Parser<'a> {
    doc: &'a mut JsonDocument,
    /// The body: read at `read`; strings are written at `write`, always behind `read`.
    input: &'a mut [u8],
    read: usize,
    /// `Latch`: the character under the cursor (0 at the end of the input or on a NUL byte);
    /// after the cursor moved on, the last one (`last()`).
    current: u8,
    loaded: bool,
    /// `StringMover`: where the next character of the string goes, where the string started.
    write: usize,
    start: usize,
    found_something: bool,
    /// `buffer_`: the characters of the number being parsed.
    number: [u8; NUMBER_CHARS],
}

type Parsed = Result<(), DeserializationError>;

impl Parser<'_> {
    /// `parse()`: the root value; a root number must end the input.
    fn parse(&mut self) -> Parsed {
        self.parse_variant(At::Root, NESTING_LIMIT)?;
        let enclosed = !matches!(
            self.doc.root.kind,
            Kind::Unsigned | Kind::Signed | Kind::Float
        );
        if self.current != 0 && !enclosed {
            return Err(DeserializationError::InvalidInput);
        }
        Ok(())
    }

    /// `Latch::current()`: reads the next character when the cursor has moved on.
    fn current(&mut self) -> u8 {
        if !self.loaded {
            self.current = match self.input.get(self.read) {
                Some(&c) => {
                    self.read += 1;
                    c
                }
                None => 0,
            };
            self.loaded = true;
        }
        self.current
    }

    /// `move()`.
    fn advance(&mut self) {
        self.loaded = false;
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.current() != c {
            return false;
        }
        self.advance();
        true
    }

    fn skip_spaces(&mut self) -> Parsed {
        loop {
            match self.current() {
                0 => {
                    return Err(if self.found_something {
                        DeserializationError::IncompleteInput
                    } else {
                        DeserializationError::EmptyInput
                    });
                }
                b' ' | b'\t' | b'\r' | b'\n' => self.advance(),
                _ => {
                    self.found_something = true;
                    return Ok(());
                }
            }
        }
    }

    fn parse_variant(&mut self, at: At, limit: u8) -> Parsed {
        self.skip_spaces()?;
        match self.current() {
            b'[' => {
                self.doc.set(at, Kind::Array, EMPTY_COLLECTION);
                self.parse_array(at, limit)
            }
            b'{' => {
                self.doc.set(at, Kind::Object, EMPTY_COLLECTION);
                self.parse_object(at, limit)
            }
            b'"' | b'\'' => self.parse_string_value(at),
            b't' => {
                self.doc.set(at, Kind::Bool, 1);
                self.skip_keyword(b"true")
            }
            b'f' => {
                self.doc.set(at, Kind::Bool, 0);
                self.skip_keyword(b"false")
            }
            // the value is not touched: a duplicate key with null keeps the earlier value
            b'n' => self.skip_keyword(b"null"),
            _ => self.parse_numeric_value(at),
        }
    }

    fn parse_array(&mut self, at: At, limit: u8) -> Parsed {
        if limit == 0 {
            return Err(DeserializationError::TooDeep);
        }
        self.advance();
        self.skip_spaces()?;
        if self.eat(b']') {
            return Ok(());
        }
        loop {
            let element = self
                .doc
                .add_slot(at)
                .ok_or(DeserializationError::NoMemory)?;
            self.parse_variant(At::Slot(element), limit - 1)?;
            self.skip_spaces()?;
            if self.eat(b']') {
                return Ok(());
            }
            if !self.eat(b',') {
                return Err(DeserializationError::InvalidInput);
            }
        }
    }

    fn parse_object(&mut self, at: At, limit: u8) -> Parsed {
        if limit == 0 {
            return Err(DeserializationError::TooDeep);
        }
        self.advance();
        self.skip_spaces()?;
        if self.eat(b'}') {
            return Ok(());
        }
        loop {
            self.parse_key()?;
            self.skip_spaces()?;
            if !self.eat(b':') {
                return Err(DeserializationError::InvalidInput);
            }
            self.terminate();
            let member = match self.doc.find_member(at, self.input, self.start) {
                Some(member) => member,
                None => {
                    let key = self.save();
                    let member = self
                        .doc
                        .add_slot(at)
                        .ok_or(DeserializationError::NoMemory)?;
                    if let Some(slot) = self.doc.slots.get_mut(usize::from(member)) {
                        slot.key = key as u32;
                    }
                    member
                }
            };
            self.parse_variant(At::Slot(member), limit - 1)?;
            self.skip_spaces()?;
            if self.eat(b'}') {
                return Ok(());
            }
            if !self.eat(b',') {
                return Err(DeserializationError::InvalidInput);
            }
            self.skip_spaces()?;
        }
    }

    fn parse_key(&mut self) -> Parsed {
        self.start_string();
        if is_quote(self.current()) {
            self.parse_quoted_string()
        } else {
            self.parse_non_quoted_string()
        }
    }

    fn parse_string_value(&mut self, at: At) -> Parsed {
        self.start_string();
        self.parse_quoted_string()?;
        let len = self.write - self.start;
        let start = self.save();
        self.doc
            .set(at, Kind::Str, start as u64 + ((len as u64) << 32));
        Ok(())
    }

    fn parse_quoted_string(&mut self) -> Parsed {
        let mut codepoint = Codepoint::default();
        let stop = self.current();
        self.advance();
        loop {
            let mut c = self.current();
            self.advance();
            if c == stop {
                return Ok(());
            }
            if c == 0 {
                return Err(DeserializationError::IncompleteInput);
            }
            if c == b'\\' {
                c = self.current();
                if c == 0 {
                    return Err(DeserializationError::IncompleteInput);
                }
                if c == b'u' {
                    self.advance();
                    let unit = self.parse_hex4()?;
                    if let Some(cp) = codepoint.append(unit) {
                        self.append_utf8(cp);
                    }
                    continue;
                }
                c = unescape(c).ok_or(DeserializationError::InvalidInput)?;
                self.advance();
            }
            self.append(c);
        }
    }

    fn parse_non_quoted_string(&mut self) -> Parsed {
        let mut c = self.current();
        if !can_be_in_non_quoted_string(c) {
            return Err(DeserializationError::InvalidInput);
        }
        while can_be_in_non_quoted_string(c) {
            self.advance();
            self.append(c);
            c = self.current();
        }
        Ok(())
    }

    fn parse_numeric_value(&mut self, at: At) -> Parsed {
        let mut n = 0;
        let mut c = self.current();
        while can_be_in_number(c) {
            let Some(slot) = self.number.get_mut(n) else {
                break;
            };
            *slot = c;
            n += 1;
            self.advance();
            c = self.current();
        }
        let number = parse_number(self.number.get(..n).unwrap_or_default())
            .ok_or(DeserializationError::InvalidInput)?;
        self.doc.set_number(at, number);
        Ok(())
    }

    fn parse_hex4(&mut self) -> Result<u16, DeserializationError> {
        let mut result = 0;
        for _ in 0..4 {
            let digit = self.current();
            if digit == 0 {
                return Err(DeserializationError::IncompleteInput);
            }
            let value = decode_hex(digit);
            if value > 0x0F {
                return Err(DeserializationError::InvalidInput);
            }
            result = (result << 4) + u16::from(value);
            self.advance();
        }
        Ok(result)
    }

    fn skip_keyword(&mut self, keyword: &[u8]) -> Parsed {
        for &expected in keyword {
            let c = self.current();
            if c == 0 {
                return Err(DeserializationError::IncompleteInput);
            }
            if c != expected {
                return Err(DeserializationError::InvalidInput);
            }
            self.advance();
        }
        Ok(())
    }

    /// `StringMover::startString()`.
    fn start_string(&mut self) {
        self.start = self.write;
    }

    /// `StringMover::append()`: the input before `read` is consumed, so `write` never
    /// overtakes an unread byte.
    fn append(&mut self, c: u8) {
        if let Some(b) = self.input.get_mut(self.write) {
            *b = c;
        }
        self.write += 1;
    }

    /// `StringMover::str()`: the NUL after the string, which stays open.
    fn terminate(&mut self) {
        if let Some(b) = self.input.get_mut(self.write) {
            *b = 0;
        }
    }

    /// `StringMover::save()`: closes the string after its NUL; returns its offset.
    fn save(&mut self) -> usize {
        self.terminate();
        self.write += 1;
        self.start
    }

    /// `Utf8::encodeCodepoint()`.
    fn append_utf8(&mut self, cp: u32) {
        if cp < 0x80 {
            self.append(cp as u8);
        } else if cp < 0x800 {
            self.append(0xC0 + (cp >> 6) as u8);
            self.append(0x80 + (cp & 0x3F) as u8);
        } else if cp < 0x1_0000 {
            self.append(0xE0 + (cp >> 12) as u8);
            self.append(0x80 + ((cp >> 6) & 0x3F) as u8);
            self.append(0x80 + (cp & 0x3F) as u8);
        } else {
            self.append(0xF0 + (cp >> 18) as u8);
            self.append(0x80 + ((cp >> 12) & 0x3F) as u8);
            self.append(0x80 + ((cp >> 6) & 0x3F) as u8);
            self.append(0x80 + (cp & 0x3F) as u8);
        }
    }
}

/// `Utf16::Codepoint` of one string.
#[derive(Default)]
struct Codepoint {
    high_surrogate: u16,
}

impl Codepoint {
    /// The code point that `unit` completes; `None` for a high surrogate, which is kept.
    fn append(&mut self, unit: u16) -> Option<u32> {
        if (0xD800..0xDC00).contains(&unit) {
            self.high_surrogate = unit & 0x3FF;
            return None;
        }
        if (0xDC00..0xE000).contains(&unit) {
            return Some(
                0x1_0000 + (u32::from(self.high_surrogate) << 10) + u32::from(unit & 0x3FF),
            );
        }
        Some(u32::from(unit))
    }
}

/// `EscapeSequence::unescapeChar()`.
fn unescape(c: u8) -> Option<u8> {
    match c {
        b'"' => Some(b'"'),
        b'\\' => Some(b'\\'),
        b'/' => Some(b'/'),
        b'b' => Some(0x08),
        b'f' => Some(0x0C),
        b'n' => Some(b'\n'),
        b'r' => Some(b'\r'),
        b't' => Some(b'\t'),
        _ => None,
    }
}

/// `decodeHex()`: a value above 15 is no hex digit. `:` .. `?` are 10..15 and the backtick is 9;
/// a byte >= 0x80 is none, whether `char` is signed or not.
fn decode_hex(c: u8) -> u8 {
    if c < b'A' {
        return c.wrapping_sub(b'0');
    }
    (c & !0x20).wrapping_sub(b'A').wrapping_add(10)
}

fn is_quote(c: u8) -> bool {
    c == b'"' || c == b'\''
}

fn can_be_in_number(c: u8) -> bool {
    c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' || c == b'e' || c == b'E'
}

fn can_be_in_non_quoted_string(c: u8) -> bool {
    c.is_ascii_digit() || (b'_'..=b'z').contains(&c) || c.is_ascii_uppercase()
}

/// `parseNumber()`: `s` is the number's text (a C string); `None` when it is no number.
fn parse_number(s: &[u8]) -> Option<Number> {
    let mut text = NumberText { s, i: 0 };
    let is_negative = text.peek() == b'-';
    if is_negative || text.peek() == b'+' {
        text.i += 1;
    }
    if !text.peek().is_ascii_digit() && text.peek() != b'.' {
        return None;
    }
    let mantissa = text.integer();
    if text.peek() == 0 {
        if !is_negative {
            return Some(Number::Unsigned(mantissa));
        }
        if mantissa <= 1 << 63 {
            return Some(Number::Signed((mantissa as i64).wrapping_neg()));
        }
    }
    let (mantissa, offset) = text.decimal(mantissa);
    let exponent = match text.exponent(offset) {
        Ok(exponent) => exponent,
        Err(beyond) => return Some(Number::Float(negated(beyond, is_negative))),
    };
    if text.peek() != 0 {
        return None;
    }
    let r = make_float(mantissa as f64, exponent);
    Some(Number::Float(negated(r, is_negative)))
}

fn negated(r: f64, is_negative: bool) -> f64 {
    if is_negative {
        -r
    } else {
        r
    }
}

/// The text of a number and the position in it.
struct NumberText<'a> {
    s: &'a [u8],
    i: usize,
}

impl NumberText<'_> {
    /// The character at the position, 0 past the end (the C string's terminator).
    fn peek(&self) -> u8 {
        self.s.get(self.i).copied().unwrap_or(0)
    }

    fn digit(&self) -> Option<u64> {
        let c = self.peek();
        if c.is_ascii_digit() {
            Some(u64::from(c - b'0'))
        } else {
            None
        }
    }

    /// The integer part as far as it fits u64.
    fn integer(&mut self) -> u64 {
        let mut mantissa: u64 = 0;
        while let Some(digit) = self.digit() {
            if mantissa > u64::MAX / 10 {
                break;
            }
            mantissa *= 10;
            if mantissa > u64::MAX - digit {
                break; // the multiplication stays: the library's factor ten
            }
            mantissa += digit;
            self.i += 1;
        }
        mantissa
    }

    /// The mantissa cut to 52 bits and the rest of the integer part and the fraction as the
    /// decimal exponent offset: `int16_t` in C++, the cut adds at most 4 and every further
    /// digit wraps the way GCC converts.
    fn decimal(&mut self, mut mantissa: u64) -> (u64, i16) {
        let mut offset: i16 = 0;
        while mantissa > MANTISSA_MAX {
            mantissa /= 10;
            offset += 1;
        }
        while self.peek().is_ascii_digit() {
            offset = offset.wrapping_add(1);
            self.i += 1;
        }
        if self.peek() == b'.' {
            self.i += 1;
            while let Some(digit) = self.digit() {
                if mantissa < MANTISSA_MAX / 10 {
                    mantissa = mantissa * 10 + digit;
                    offset = offset.wrapping_sub(1);
                }
                self.i += 1;
            }
        }
        (mantissa, offset)
    }

    /// The exponent plus `offset`; `Err` with 0.0 or inf as soon as it passes 308.
    fn exponent(&mut self, offset: i16) -> Result<i32, f64> {
        let mut exponent: i32 = 0;
        if self.peek() == b'e' || self.peek() == b'E' {
            self.i += 1;
            let negative = self.peek() == b'-';
            if negative || self.peek() == b'+' {
                self.i += 1;
            }
            while let Some(digit) = self.digit() {
                exponent = exponent * 10 + digit as i32;
                if exponent + i32::from(offset) > EXPONENT_MAX {
                    return Err(if negative { 0.0 } else { f64::INFINITY });
                }
                self.i += 1;
            }
            if negative {
                exponent = -exponent;
            }
        }
        Ok(exponent + i32::from(offset))
    }
}

/// `make_float()`: `m` times 10^`e` by binary exponentiation over the tables, NaN when `e`
/// needs a power beyond 1e256.
fn make_float(m: f64, e: i32) -> f64 {
    // `e > 0` in C++; for 0 the table is not read
    let powers = if e.is_positive() {
        &POSITIVE_POWERS
    } else {
        &NEGATIVE_POWERS
    };
    let mut e = e.unsigned_abs();
    let mut m = m;
    let mut index = 0;
    while e != 0 {
        let Some(&power) = powers.get(index) else {
            return f64::from_bits(NAN_BITS);
        };
        if e & 1 != 0 {
            m *= f64::from_bits(power);
        }
        e >>= 1;
        index += 1;
    }
    m
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_golden;
