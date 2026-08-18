use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;

const ENDPOINT_FILE: &str = ".pinvou-qwen3-asr-service.endpoint";
const PATH_PROTOCOL: &str = "pinvou-qwen3-asr-v1";
const AUDIO_PROTOCOL: &str = "pinvou-qwen3-asr-audio-v3";
const AUDIO_REQUEST: &str = "AUDIO3";
const WARMUP_PROTOCOL: &str = "pinvou-qwen3-asr-warmup-v1";
const WARMUP_REQUEST: &str = "WARM2";
const WARMUP_BUCKET_SECONDS: u8 = 10;
const MAX_AUDIO_BYTES: usize = 2 * 1024 * 1024;
const MAX_CONTEXT_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    port: u16,
    token: String,
    warmup_supported: bool,
}

fn parse_endpoint(raw: &str) -> Result<Endpoint, String> {
    let values = raw
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect::<HashMap<_, _>>();
    if values.get("protocol").copied() != Some(PATH_PROTOCOL) {
        return Err("Qwen3-ASR resident service path protocol is incompatible".to_string());
    }
    if values.get("audio_protocol").copied() != Some(AUDIO_PROTOCOL) {
        return Err("Qwen3-ASR resident service does not support direct audio bytes".to_string());
    }
    let port = values
        .get("port")
        .ok_or_else(|| "Qwen3-ASR resident endpoint has no port".to_string())?
        .parse::<u16>()
        .map_err(|error| format!("Qwen3-ASR resident endpoint port is invalid: {error}"))?;
    if port == 0 {
        return Err("Qwen3-ASR resident endpoint port is zero".to_string());
    }
    let token = values
        .get("token")
        .map(|value| (*value).to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Qwen3-ASR resident endpoint has no token".to_string())?;
    let warmup_supported = values.get("warmup_protocol").copied() == Some(WARMUP_PROTOCOL);
    Ok(Endpoint {
        port,
        token,
        warmup_supported,
    })
}

fn read_response_line(stream: &mut TcpStream) -> Result<String, String> {
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let count = stream
            .read(&mut chunk)
            .map_err(|error| format!("read Qwen3-ASR resident response: {error}"))?;
        if count == 0 {
            return Err("Qwen3-ASR resident service closed without a response".to_string());
        }
        if let Some(newline) = chunk[..count].iter().position(|byte| *byte == b'\n') {
            response.extend_from_slice(&chunk[..newline]);
            if response.len() > MAX_RESPONSE_BYTES {
                return Err("Qwen3-ASR resident response exceeds 64 KiB".to_string());
            }
            break;
        }
        response.extend_from_slice(&chunk[..count]);
        if response.len() > MAX_RESPONSE_BYTES {
            return Err("Qwen3-ASR resident response exceeds 64 KiB".to_string());
        }
    }
    String::from_utf8(response)
        .map_err(|error| format!("Qwen3-ASR resident response is not UTF-8: {error}"))
}

fn parse_response(response: &str) -> Result<String, String> {
    let fields = response.split('\t').collect::<Vec<_>>();
    if fields.len() >= 2 && fields[0] == "ERR" {
        let message = base64::engine::general_purpose::STANDARD
            .decode(fields[1])
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .filter(|message| !message.trim().is_empty())
            .unwrap_or_else(|| "Qwen3-ASR resident service rejected the request".to_string());
        return Err(message);
    }
    if fields.len() < 5 || fields[0] != "OK" {
        return Err("Qwen3-ASR resident service returned an invalid response".to_string());
    }
    let text = base64::engine::general_purpose::STANDARD
        .decode(fields[1])
        .map_err(|error| format!("decode Qwen3-ASR transcript: {error}"))
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|error| format!("Qwen3-ASR transcript is not UTF-8: {error}"))
        })?;
    if text.trim().is_empty() {
        return Err("Qwen3-ASR resident service returned an empty transcript".to_string());
    }
    Ok(text)
}

fn transcribe_endpoint(
    endpoint: &Endpoint,
    audio_bytes: &[u8],
    language: &str,
    context: &str,
    max_new_tokens: u16,
    timeout: Duration,
) -> Result<String, String> {
    if !(44..=MAX_AUDIO_BYTES).contains(&audio_bytes.len()) {
        return Err(format!(
            "Qwen3-ASR WAV must contain between 44 and {MAX_AUDIO_BYTES} bytes"
        ));
    }
    if max_new_tokens == 0 || max_new_tokens > 256 {
        return Err("Qwen3-ASR max_new_tokens must be between 1 and 256".to_string());
    }
    if context.len() > MAX_CONTEXT_BYTES {
        return Err(format!(
            "Qwen3-ASR context exceeds {MAX_CONTEXT_BYTES} UTF-8 bytes"
        ));
    }
    let language = base64::engine::general_purpose::STANDARD.encode(language.as_bytes());
    let context = base64::engine::general_purpose::STANDARD.encode(context.as_bytes());
    let header = format!(
        "{AUDIO_REQUEST}\t{}\t{language}\t{max_new_tokens}\t{context}\t{}\n",
        endpoint.token,
        audio_bytes.len()
    );
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port);
    let mut stream = TcpStream::connect_timeout(&address.into(), CONNECT_TIMEOUT)
        .map_err(|error| format!("connect Qwen3-ASR resident service: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(5))))
        .map_err(|error| format!("configure Qwen3-ASR resident socket: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("configure Qwen3-ASR resident TCP_NODELAY: {error}"))?;
    stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(audio_bytes))
        .and_then(|()| stream.flush())
        .map_err(|error| format!("send WAV to Qwen3-ASR resident service: {error}"))?;
    parse_response(&read_response_line(&mut stream)?)
}

fn prewarm_endpoint(endpoint: &Endpoint, timeout: Duration) -> Result<bool, String> {
    if !endpoint.warmup_supported {
        return Ok(false);
    }
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port);
    let mut stream = TcpStream::connect_timeout(&address.into(), CONNECT_TIMEOUT)
        .map_err(|error| format!("connect Qwen3-ASR resident service for prewarm: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(2))))
        .map_err(|error| format!("configure Qwen3-ASR prewarm socket: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("configure Qwen3-ASR prewarm TCP_NODELAY: {error}"))?;
    let request = format!(
        "{WARMUP_REQUEST}\t{}\t{WARMUP_BUCKET_SECONDS}\n",
        endpoint.token
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.flush())
        .map_err(|error| format!("send Qwen3-ASR prewarm request: {error}"))?;
    let response = read_response_line(&mut stream)?;
    let fields = response.split('\t').collect::<Vec<_>>();
    match fields.first().copied() {
        Some("WARMED") => Ok(true),
        Some("SKIPPED") => Ok(false),
        Some("ERR") => parse_response(&response).map(|_| false),
        _ => Err("Qwen3-ASR resident service returned an invalid prewarm response".to_string()),
    }
}

pub(crate) fn transcribe(
    cache_dir: &Path,
    audio_bytes: &[u8],
    language: &str,
    context: &str,
    timeout: Duration,
) -> Result<String, String> {
    let endpoint_path = cache_dir.join(ENDPOINT_FILE);
    let raw = std::fs::read_to_string(&endpoint_path).map_err(|error| {
        format!(
            "read Qwen3-ASR resident endpoint {}: {error}",
            endpoint_path.display()
        )
    })?;
    let endpoint = parse_endpoint(&raw)?;
    transcribe_endpoint(&endpoint, audio_bytes, language, context, 256, timeout)
}

pub(crate) fn prewarm(cache_dir: &Path, timeout: Duration) -> Result<bool, String> {
    let endpoint_path = cache_dir.join(ENDPOINT_FILE);
    let raw = std::fs::read_to_string(&endpoint_path).map_err(|error| {
        format!(
            "read Qwen3-ASR resident endpoint {}: {error}",
            endpoint_path.display()
        )
    })?;
    prewarm_endpoint(&parse_endpoint(&raw)?, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parses_audio_v3_endpoint_without_exposing_token() {
        let endpoint = parse_endpoint(
            "protocol=pinvou-qwen3-asr-v1\n\
             audio_protocol=pinvou-qwen3-asr-audio-v3\n\
             warmup_protocol=pinvou-qwen3-asr-warmup-v1\n\
             port=61381\n\
             token=secret-value\n",
        )
        .expect("audio v2 endpoint");
        assert_eq!(endpoint.port, 61_381);
        assert_eq!(endpoint.token, "secret-value");
        assert!(endpoint.warmup_supported);
    }

    #[test]
    fn rejects_legacy_endpoint_so_caller_can_use_cli_fallback() {
        let error = parse_endpoint("protocol=pinvou-qwen3-asr-v1\nport=61381\ntoken=legacy\n")
            .expect_err("legacy endpoint must use fallback");
        assert!(error.contains("direct audio bytes"));
    }

    #[test]
    fn sends_framed_wav_bytes_with_context_and_decodes_transcript() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let endpoint = Endpoint {
            port: listener.local_addr().expect("address").port(),
            token: "local-test-token".to_string(),
            warmup_supported: true,
        };
        let audio = vec![7_u8; 128];
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("client");
            let mut reader = std::io::BufReader::new(stream);
            let mut header = String::new();
            reader.read_line(&mut header).expect("header");
            let context =
                base64::engine::general_purpose::STANDARD.encode("相关词：PinvouOS、Qwen3-ASR");
            assert_eq!(
                header,
                format!("AUDIO3\tlocal-test-token\temg=\t256\t{context}\t128\n")
            );
            let mut payload = vec![0_u8; 128];
            reader.read_exact(&mut payload).expect("payload");
            assert_eq!(payload, vec![7_u8; 128]);
            let transcript = base64::engine::general_purpose::STANDARD.encode("测试成功");
            reader
                .get_mut()
                .write_all(format!("OK\t{transcript}\t5.0\t0.5\t0.6\n").as_bytes())
                .expect("response");
        });
        let text = transcribe_endpoint(
            &endpoint,
            &audio,
            "zh",
            "相关词：PinvouOS、Qwen3-ASR",
            256,
            Duration::from_secs(2),
        )
        .expect("transcript");
        server.join().expect("server");
        assert_eq!(text, "测试成功");
    }

    #[test]
    fn prewarm_is_skipped_when_audio_v3_service_lacks_warmup() {
        let endpoint = parse_endpoint(
            "protocol=pinvou-qwen3-asr-v1\n\
             audio_protocol=pinvou-qwen3-asr-audio-v3\n\
             port=61381\n\
             token=audio-v3-without-warmup\n",
        )
        .expect("audio endpoint");
        assert!(!endpoint.warmup_supported);
        assert!(!prewarm_endpoint(&endpoint, Duration::from_millis(1)).expect("skip"));
    }

    #[test]
    fn sends_idle_prewarm_request() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener");
        let endpoint = Endpoint {
            port: listener.local_addr().expect("address").port(),
            token: "warm-token".to_string(),
            warmup_supported: true,
        };
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client");
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut header = String::new();
            reader.read_line(&mut header).expect("header");
            assert_eq!(header, "WARM2\twarm-token\t10\n");
            stream
                .write_all(b"SKIPPED\trecent-inference\t2.000\n")
                .expect("response");
        });
        assert!(!prewarm_endpoint(&endpoint, Duration::from_secs(2)).expect("prewarm"));
        server.join().expect("server");
    }
}
