const VARINT: u32 = 0;
const FIXED64: u32 = 1;
const BYTES: u32 = 2;
const FIXED32: u32 = 5;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    fn varint(&mut self, mut value: u64) {
        loop {
            let low = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                self.0.push(low);
                return;
            }
            self.0.push(low | 0x80);
        }
    }

    fn key(&mut self, field: u32, kind: u32) {
        self.varint(u64::from(field) << 3 | u64::from(kind));
    }

    pub fn uint(mut self, field: u32, value: u64) -> Self {
        self.key(field, VARINT);
        self.varint(value);
        self
    }

    pub fn flag(self, field: u32, value: bool) -> Self {
        self.uint(field, u64::from(value))
    }

    pub fn float(mut self, field: u32, value: f32) -> Self {
        self.key(field, FIXED32);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub fn bytes(mut self, field: u32, value: &[u8]) -> Self {
        self.key(field, BYTES);
        self.varint(value.len() as u64);
        self.0.extend_from_slice(value);
        self
    }

    pub fn text(self, field: u32, value: &str) -> Self {
        self.bytes(field, value.as_bytes())
    }

    pub fn message(self, field: u32, value: Writer) -> Self {
        self.bytes(field, &value.0)
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    Uint(u64),
    Fixed32([u8; 4]),
    Fixed64([u8; 8]),
    Bytes(&'a [u8]),
}

impl<'a> Value<'a> {
    pub fn uint(self) -> Option<u64> {
        match self {
            Self::Uint(value) => Some(value),
            _ => None,
        }
    }

    pub fn bytes(self) -> Option<&'a [u8]> {
        match self {
            Self::Bytes(value) => Some(value),
            _ => None,
        }
    }

    pub fn text(self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }

    pub fn float(self) -> Option<f32> {
        match self {
            Self::Fixed32(raw) => Some(f32::from_le_bytes(raw)),
            _ => None,
        }
    }
}

pub struct Fields<'a> {
    rest: &'a [u8],
}

pub fn fields(buffer: &[u8]) -> Fields<'_> {
    Fields { rest: buffer }
}

impl<'a> Fields<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = self.rest.split_first()?;
            self.rest = rest;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.rest.split_at_checked(count)?;
        self.rest = rest;
        Some(head)
    }

    fn field(&mut self) -> Option<(u32, Value<'a>)> {
        let key = self.varint()?;
        let field = u32::try_from(key >> 3).ok()?;
        let value = match (key & 7) as u32 {
            VARINT => Value::Uint(self.varint()?),
            FIXED64 => Value::Fixed64(self.take(8)?.try_into().ok()?),
            BYTES => {
                let length = usize::try_from(self.varint()?).ok()?;
                Value::Bytes(self.take(length)?)
            }
            FIXED32 => Value::Fixed32(self.take(4)?.try_into().ok()?),
            _ => return None,
        };
        Some((field, value))
    }
}

impl<'a> Iterator for Fields<'a> {
    type Item = (u32, Value<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let field = self.field();
        if field.is_none() {
            self.rest = &[];
        }
        field
    }
}

pub fn fields_packed(buffer: &[u8]) -> Vec<u64> {
    let mut reader = Fields { rest: buffer };
    let mut values = Vec::new();
    while !reader.rest.is_empty() {
        match reader.varint() {
            Some(value) => values.push(value),
            None => break,
        }
    }
    values
}

pub fn find<'a>(buffer: &'a [u8], field: u32) -> Option<Value<'a>> {
    fields(buffer)
        .find(|(number, _)| *number == field)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_is_read_back_field_by_field() {
        let inner = Writer::new().text(1, "Vignette").uint(2, 300);
        let buffer = Writer::new()
            .uint(1, 7)
            .flag(2, true)
            .float(3, 0.5)
            .message(4, inner)
            .finish();
        let read: Vec<_> = fields(&buffer).collect();
        assert_eq!(read.len(), 4);
        assert_eq!(read[0], (1, Value::Uint(7)));
        assert_eq!(read[1], (2, Value::Uint(1)));
        assert_eq!(read[2].1.float(), Some(0.5));
        let nested = read[3].1.bytes().unwrap();
        assert_eq!(find(nested, 1).and_then(Value::text), Some("Vignette"));
        assert_eq!(find(nested, 2).and_then(Value::uint), Some(300));
    }

    #[test]
    fn large_numbers_take_several_bytes_and_survive() {
        let buffer = Writer::new().uint(1, u64::MAX).uint(2, 128).finish();
        assert_eq!(find(&buffer, 1).and_then(Value::uint), Some(u64::MAX));
        assert_eq!(find(&buffer, 2).and_then(Value::uint), Some(128));
    }

    #[test]
    fn a_cut_off_message_ends_quietly_instead_of_inventing_fields() {
        let buffer = Writer::new().text(1, "Sharpen").text(2, "Details").finish();
        let cut = &buffer[..buffer.len() - 3];
        let read: Vec<_> = fields(cut).collect();
        assert_eq!(read, vec![(1, Value::Bytes(b"Sharpen"))]);
    }
}
