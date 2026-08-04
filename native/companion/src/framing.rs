use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

const MAX_PHYSICAL_CHUNK_BYTES: usize = 1024 * 1024;
const CHUNK_KIND_SHIFT: u32 = 30;
const CHUNK_LENGTH_MASK: u32 = (1 << CHUNK_KIND_SHIFT) - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChunkKind {
    Single = 0,
    Start = 1,
    Continue = 2,
    End = 3,
}

impl ChunkKind {
    fn decode(header: u32) -> io::Result<Self> {
        match header >> CHUNK_KIND_SHIFT {
            0 => Ok(Self::Single),
            1 => Ok(Self::Start),
            2 => Ok(Self::Continue),
            3 => Ok(Self::End),
            _ => unreachable!("the two-bit chunk kind is exhaustive"),
        }
    }
}

pub(crate) fn read_json_record<T: DeserializeOwned>(
    reader: &mut impl Read,
) -> io::Result<Option<T>> {
    let Some((kind, first)) = read_chunk(reader)? else {
        return Ok(None);
    };
    let payload = match kind {
        ChunkKind::Single => first,
        ChunkKind::Start => {
            let mut record = first;
            loop {
                let Some((next_kind, chunk)) = read_chunk(reader)? else {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "companion record ended before its final chunk",
                    ));
                };
                match next_kind {
                    ChunkKind::Continue => record.extend_from_slice(&chunk),
                    ChunkKind::End => {
                        record.extend_from_slice(&chunk);
                        break;
                    }
                    ChunkKind::Single | ChunkKind::Start => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "companion record contains an invalid chunk sequence",
                        ));
                    }
                }
            }
            record
        }
        ChunkKind::Continue | ChunkKind::End => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "companion record starts with a continuation chunk",
            ));
        }
    };
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(io::Error::other)
}

pub(crate) fn write_json_record<T: Serialize>(
    writer: &mut impl Write,
    value: &T,
) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "companion record cannot be empty",
        ));
    }
    if payload.len() <= MAX_PHYSICAL_CHUNK_BYTES {
        return write_chunk(writer, ChunkKind::Single, &payload);
    }
    let count = payload.len().div_ceil(MAX_PHYSICAL_CHUNK_BYTES);
    for (index, chunk) in payload.chunks(MAX_PHYSICAL_CHUNK_BYTES).enumerate() {
        let kind = if index == 0 {
            ChunkKind::Start
        } else if index + 1 == count {
            ChunkKind::End
        } else {
            ChunkKind::Continue
        };
        write_chunk(writer, kind, chunk)?;
    }
    Ok(())
}

fn read_chunk(reader: &mut impl Read) -> io::Result<Option<(ChunkKind, Vec<u8>)>> {
    let mut header = [0_u8; 4];
    match reader.read(&mut header[..1])? {
        0 => return Ok(None),
        1 => reader.read_exact(&mut header[1..])?,
        _ => unreachable!("one-byte read cannot return more than one byte"),
    }
    let header = u32::from_le_bytes(header);
    let kind = ChunkKind::decode(header)?;
    let length = (header & CHUNK_LENGTH_MASK) as usize;
    if !(1..=MAX_PHYSICAL_CHUNK_BYTES).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "companion physical chunk has an invalid size",
        ));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some((kind, payload)))
}

fn write_chunk(writer: &mut impl Write, kind: ChunkKind, payload: &[u8]) -> io::Result<()> {
    if !(1..=MAX_PHYSICAL_CHUNK_BYTES).contains(&payload.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "companion physical chunk has an invalid size",
        ));
    }
    let header = ((kind as u32) << CHUNK_KIND_SHIFT) | payload.len() as u32;
    writer.write_all(&header.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_json_record_round_trips_through_chunk_sequence() {
        let value = serde_json::json!({
            "type": "message",
            "v": 5,
            "target": "broadcast",
            "payload": "x".repeat(MAX_PHYSICAL_CHUNK_BYTES + 257),
        });
        let mut encoded = Vec::new();
        write_json_record(&mut encoded, &value).unwrap();
        let decoded = read_json_record::<serde_json::Value>(&mut encoded.as_slice())
            .unwrap()
            .unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn continuation_cannot_start_a_record() {
        let mut encoded = Vec::new();
        write_chunk(&mut encoded, ChunkKind::Continue, b"{}").unwrap();
        assert_eq!(
            read_json_record::<serde_json::Value>(&mut encoded.as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData,
        );
    }
}
