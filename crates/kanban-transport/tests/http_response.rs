#[path = "common/http_response.rs"]
mod http_response;

use http_response::read_response;
use std::io::{self, Read};

struct Reset;
impl Read for Reset {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::ErrorKind::ConnectionReset.into())
    }
}

#[test]
fn complete_refusal_does_not_require_transport_eof() {
    let wire = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\n\r\nrequest refused";
    let mut reader = wire.as_slice().chain(Reset);
    let response = read_response(&mut reader).expect("complete HTTP refusal before reset");
    assert_eq!(response.as_bytes(), wire);
}

#[test]
fn complete_early_refusal_allows_a_late_request_body() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let request = b"POST /mcp HTTP/1.1\r\nHost: fixture\r\nContent-Length: 2\r\n\r\n";
    let wire = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\n\r\nrequest refused";
    let peer = std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut headers = vec![0; request.len()];
        stream.read_exact(&mut headers).unwrap();
        assert_eq!(headers, request);
        for byte in wire {
            stream.write_all(&[*byte]).unwrap();
        }
        let mut late_body = [0; 2];
        stream.read_exact(&mut late_body).unwrap();
        assert_eq!(&late_body, b"{}");
    });
    use std::io::Write;
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    stream.write_all(request).unwrap();
    let response = read_response(&mut stream);
    stream.write_all(b"{}").unwrap();
    peer.join().unwrap();
    assert_eq!(
        response
            .expect("complete refusal before late body")
            .as_bytes(),
        wire
    );
}

#[test]
fn fragmented_response_waits_for_the_complete_body() {
    let wire =
        b"HTTP/1.1 403 Forbidden\r\ncOnTeNt-LeNgTh: 15\r\nX-Fixture: yes\r\n\r\nrequest refused";
    for split in 0..=wire.len() {
        let mut reader = wire[..split].chain(&wire[split..]).chain(Reset);
        let response = read_response(&mut reader).expect("fragmented HTTP refusal before reset");
        assert_eq!(response.as_bytes(), wire, "split {split}");
    }
}

#[test]
fn reset_before_response_completion_is_an_error() {
    let wire = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\n\r\nrequest refused";
    for end in 0..wire.len() {
        let mut reader = wire[..end].chain(Reset);
        assert_eq!(
            read_response(&mut reader).unwrap_err().kind(),
            io::ErrorKind::ConnectionReset,
            "prefix {end}"
        );
    }
}

#[test]
fn eof_before_response_completion_is_an_error() {
    let wire = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\n\r\nrequest refused";
    for end in 0..wire.len() {
        assert_eq!(
            read_response(&mut &wire[..end]).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof,
            "prefix {end}"
        );
    }
}

#[test]
fn malformed_or_unsupported_framing_is_an_error() {
    for wire in [
        "NOT-HTTP 401 Unauthorized\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 4010 Unauthorized\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nBad Header: value\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nBad: value\0\r\nContent-Length: 0\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: nope\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: -1\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: +1\r\n\r\nx",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length:\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 1, 1\r\n\r\nx",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nx",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 99999999999999999999999999\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
    ] {
        assert_eq!(
            read_response(&mut wire.as_bytes()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn response_header_and_body_sizes_are_bounded() {
    for wire in [
        format!(
            "HTTP/1.1 401 Unauthorized\r\nX-Large: {}",
            "x".repeat(16 * 1024)
        ),
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 4194305\r\n\r\n".into(),
    ] {
        let mut reader = wire.as_bytes().chain(Reset);
        assert_eq!(
            read_response(&mut reader).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
