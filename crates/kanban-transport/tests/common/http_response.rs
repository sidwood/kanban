// cspell:ignore httparse
use std::io::{self, Read};

// Loopback responses use Full bodies, including collected SDK responses.
// Reject other framing instead of treating a transport close as completion.
pub fn read_response(reader: &mut impl Read) -> io::Result<String> {
    const MAX_HEADER_BYTES: usize = 16 * 1024;
    const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP response framing");
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        if response.len() == MAX_HEADER_BYTES {
            return Err(invalid());
        }
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        response.push(byte[0]);
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut parsed = httparse::Response::new(&mut headers);
    if parsed.parse(&response).map_err(|_| invalid())? != httparse::Status::Complete(response.len())
        || !matches!(parsed.code, Some(200..=599))
    {
        return Err(invalid());
    }
    let mut body_length = None;
    for header in parsed.headers {
        if header.name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(invalid());
        }
        if header.name.eq_ignore_ascii_case("content-length") {
            if body_length.is_some()
                || header.value.is_empty()
                || !header.value.iter().all(u8::is_ascii_digit)
            {
                return Err(invalid());
            }
            body_length = Some(
                std::str::from_utf8(header.value)
                    .map_err(|_| invalid())?
                    .parse::<usize>()
                    .map_err(|_| invalid())?,
            );
        }
    }
    let body_length = body_length
        .filter(|length| *length <= MAX_BODY_BYTES)
        .ok_or_else(invalid)?;
    let header_length = response.len();
    response.resize(header_length + body_length, 0);
    reader.read_exact(&mut response[header_length..])?;
    String::from_utf8(response).map_err(|_| invalid())
}
