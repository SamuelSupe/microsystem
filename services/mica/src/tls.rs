extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::num::NonZeroU32;

use embedded_io::{ErrorKind, ErrorType, Read, Write};
use embedded_tls::blocking::TlsConnection;
use embedded_tls::pki::CertVerifier;
use embedded_tls::{
    Aes128GcmSha256, Certificate, CertificateEntryRef, CertificateRef, CertificateVerifyRef,
    CryptoProvider, SignatureScheme, TlsClock, TlsConfig, TlsContext, TlsError, TlsVerifier,
};
use microsystem_abi::CapHandle;
use microsystem_mica::ErrorValue;
use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};

const BUNDLE_MAGIC: &[u8; 4] = b"MCAB";
const BUNDLE_VERSION: u32 = 1;
const BUNDLE_SHA256: [u8; 32] = parse_sha256(match option_env!("MICROSYSTEM_CA_BUNDLE_SHA256") {
    Some(value) => value,
    None => "328fa09c4231bdf284b873b2b1e19c82901e12293960a4f9f1ea939ab7cca2b6",
});
const TLS_RECORD_BYTES: usize = 16_640;
const TLS_RESPONSE_BYTES: usize = 1024 * 1024 + 16 * 1024;
const CERTIFICATE_BYTES: usize = 16 * 1024;

pub trait NetworkIo {
    fn connect(&mut self, host: &str, port: u16) -> Result<u64, ErrorValue>;
    fn read(&mut self, connection: u64, output: &mut [u8]) -> Result<usize, ErrorValue>;
    fn write(&mut self, connection: u64, input: &[u8]) -> Result<usize, ErrorValue>;
    fn close(&mut self, connection: u64) -> Result<(), ErrorValue>;
}

pub fn exchange<H: NetworkIo>(
    host: &mut H,
    random: CapHandle,
    server_name: &str,
    port: u16,
    request: &[u8],
    bundle: &[u8],
) -> Result<(u64, Vec<u8>), ErrorValue> {
    if random == CapHandle::INVALID {
        return Err(ErrorValue::new("tls", "TLS random source is unavailable"));
    }
    if server_name.is_empty() || server_name.len() > 253 || !server_name.is_ascii() {
        return Err(ErrorValue::new("tls", "TLS server name is invalid"));
    }
    microsystem_user_rt::clock_realtime()
        .map_err(|_| ErrorValue::new("tls", "trusted realtime clock is unavailable"))?;
    let roots = parse_bundle(bundle)?;
    let connection = host.connect(server_name, port)?;
    let stream = NetworkStream { host, connection };
    let mut read_buffer = alloc::vec![0u8; TLS_RECORD_BYTES];
    let mut write_buffer = alloc::vec![0u8; TLS_RECORD_BYTES];
    let mut tls = TlsConnection::new(stream, &mut read_buffer, &mut write_buffer);
    let config = TlsConfig::new().with_server_name(server_name);
    let provider = VerifiedProvider {
        rng: KernelRng { source: random },
        verifier: MozillaVerifier::new(roots),
    };
    if let Err(error) = tls.open(TlsContext::new(&config, provider)) {
        let stream = match tls.close() {
            Ok(stream) | Err((stream, _)) => stream,
        };
        let _ = stream.host.close(stream.connection);
        return Err(ErrorValue::new(
            "tls",
            alloc::format!("TLS handshake validation failed: {error:?}"),
        ));
    }
    let result = (|| {
        let mut written = 0usize;
        while written < request.len() {
            let bytes = tls
                .write(&request[written..])
                .map_err(|_| ErrorValue::new("tls", "TLS request write failed"))?;
            if bytes == 0 {
                return Err(ErrorValue::new("tls", "TLS request write made no progress"));
            }
            written += bytes;
        }
        tls.flush()
            .map_err(|_| ErrorValue::new("tls", "TLS request flush failed"))?;
        let mut response = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let bytes = match tls.read(&mut chunk) {
                Ok(bytes) => bytes,
                Err(TlsError::ConnectionClosed) => break,
                Err(TlsError::IoError) if http_message_complete(&response) => break,
                Err(error) => {
                    return Err(ErrorValue::new(
                        "tls",
                        alloc::format!("TLS response read failed: {error:?}"),
                    ));
                }
            };
            if bytes == 0 {
                break;
            }
            if response.len().saturating_add(bytes) > TLS_RESPONSE_BYTES {
                return Err(ErrorValue::new(
                    "limit",
                    "HTTPS response exceeds the 1 MiB default limit",
                ));
            }
            response.extend_from_slice(&chunk[..bytes]);
        }
        Ok(response)
    })();
    let stream = match tls.close() {
        Ok(stream) | Err((stream, _)) => stream,
    };
    let _ = stream.host.close(stream.connection);
    result.map(|response| (connection, response))
}

fn http_message_complete(response: &[u8]) -> bool {
    let Some(header_offset) = response.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
        return false;
    };
    let header_end = header_offset + 4;
    let Ok(headers) = core::str::from_utf8(&response[..header_end]) else {
        return false;
    };
    let body = &response[header_end..];
    let mut content_length = None;
    let mut chunked = false;
    for line in headers.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse::<usize>().ok();
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.trim().eq_ignore_ascii_case("chunked")
        {
            chunked = true;
        }
    }
    content_length.is_some_and(|length| body.len() >= length)
        || (chunked && complete_chunked_body(body))
}

fn complete_chunked_body(body: &[u8]) -> bool {
    let mut cursor = 0usize;
    loop {
        let Some(line_offset) = body[cursor..].windows(2).position(|bytes| bytes == b"\r\n") else {
            return false;
        };
        let line_end = cursor + line_offset;
        let Ok(line) = core::str::from_utf8(&body[cursor..line_end]) else {
            return false;
        };
        let Ok(length) = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
        else {
            return false;
        };
        cursor = line_end + 2;
        if length == 0 {
            return true;
        }
        let Some(end) = cursor.checked_add(length) else {
            return false;
        };
        if end + 2 > body.len() || &body[end..end + 2] != b"\r\n" {
            return false;
        }
        cursor = end + 2;
    }
}

fn parse_bundle(bundle: &[u8]) -> Result<Vec<&[u8]>, ErrorValue> {
    if bundle.len() < 16
        || &bundle[..4] != BUNDLE_MAGIC
        || u32::from_le_bytes(bundle[4..8].try_into().unwrap()) != BUNDLE_VERSION
        || Sha256::digest(bundle)[..] != BUNDLE_SHA256
    {
        return Err(ErrorValue::new("tls", "CA bundle integrity check failed"));
    }
    let count = u32::from_le_bytes(bundle[8..12].try_into().unwrap()) as usize;
    let payload = u32::from_le_bytes(bundle[12..16].try_into().unwrap()) as usize;
    if count == 0 || count > 512 || payload != bundle.len() - 16 {
        return Err(ErrorValue::new("tls", "CA bundle header is invalid"));
    }
    let mut roots = Vec::with_capacity(count);
    let mut cursor = 16usize;
    while cursor < bundle.len() {
        let length_end = cursor
            .checked_add(4)
            .filter(|end| *end <= bundle.len())
            .ok_or_else(|| ErrorValue::new("tls", "CA bundle is truncated"))?;
        let bytes = u32::from_le_bytes(bundle[cursor..length_end].try_into().unwrap()) as usize;
        cursor = length_end;
        let end = cursor
            .checked_add(bytes)
            .filter(|end| *end <= bundle.len())
            .ok_or_else(|| ErrorValue::new("tls", "CA certificate is truncated"))?;
        if bytes == 0 {
            return Err(ErrorValue::new("tls", "CA certificate is empty"));
        }
        roots.push(&bundle[cursor..end]);
        cursor = end;
    }
    if roots.len() != count {
        return Err(ErrorValue::new("tls", "CA bundle count is invalid"));
    }
    Ok(roots)
}

const fn parse_sha256(value: &str) -> [u8; 32] {
    let bytes = value.as_bytes();
    assert!(
        bytes.len() == 64,
        "CA bundle SHA-256 must contain 64 hex digits"
    );
    let mut output = [0u8; 32];
    let mut index = 0usize;
    while index < output.len() {
        output[index] = (hex_digit(bytes[index * 2]) << 4) | hex_digit(bytes[index * 2 + 1]);
        index += 1;
    }
    output
}

const fn hex_digit(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => panic!("CA bundle SHA-256 contains a non-hex digit"),
    }
}

struct NetworkStream<'a, H> {
    host: &'a mut H,
    connection: u64,
}

#[derive(Debug)]
struct NetworkError;

impl core::fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("network I/O failed")
    }
}

impl core::error::Error for NetworkError {}

impl embedded_io::Error for NetworkError {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

impl<H> ErrorType for NetworkStream<'_, H> {
    type Error = NetworkError;
}

impl<H: NetworkIo> Read for NetworkStream<'_, H> {
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        self.host
            .read(self.connection, output)
            .map_err(|_| NetworkError)
    }
}

impl<H: NetworkIo> Write for NetworkStream<'_, H> {
    fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        self.host
            .write(self.connection, input)
            .map_err(|_| NetworkError)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

struct KernelRng {
    source: CapHandle,
}

impl RngCore for KernelRng {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0u8; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }

    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    fn fill_bytes(&mut self, output: &mut [u8]) {
        if self.try_fill_bytes(output).is_err() {
            panic!("kernel random source failed");
        }
    }

    fn try_fill_bytes(&mut self, output: &mut [u8]) -> Result<(), rand_core::Error> {
        for chunk in output.chunks_mut(256) {
            microsystem_user_rt::random_fill(self.source, chunk).map_err(|_| {
                rand_core::Error::from(NonZeroU32::new(1).expect("non-zero random error"))
            })?;
        }
        Ok(())
    }
}

impl CryptoRng for KernelRng {}

struct RtcClock;

impl TlsClock for RtcClock {
    fn now() -> Option<u64> {
        microsystem_user_rt::clock_realtime().ok()
    }
}

type RootVerifier<'a> = CertVerifier<'a, Aes128GcmSha256, RtcClock, CERTIFICATE_BYTES>;
type HandshakeHash = <Aes128GcmSha256 as embedded_tls::TlsCipherSuite>::Hash;

struct MozillaVerifier<'a> {
    roots: Vec<&'a [u8]>,
    hostname: String,
    selected: Option<Box<RootVerifier<'a>>>,
    leaf: Vec<u8>,
    transcript: Option<HandshakeHash>,
}

impl<'a> MozillaVerifier<'a> {
    fn new(roots: Vec<&'a [u8]>) -> Self {
        Self {
            roots,
            hostname: String::new(),
            selected: None,
            leaf: Vec::new(),
            transcript: None,
        }
    }
}

impl TlsVerifier<Aes128GcmSha256> for MozillaVerifier<'_> {
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        if hostname.len() > 253 {
            return Err(TlsError::InsufficientSpace);
        }
        self.hostname.clear();
        self.hostname.push_str(hostname);
        Ok(())
    }

    fn verify_certificate(
        &mut self,
        transcript: &<Aes128GcmSha256 as embedded_tls::TlsCipherSuite>::Hash,
        certificate: CertificateRef<'_>,
    ) -> Result<(), TlsError> {
        let presented_root = match certificate.entries.last() {
            Some(CertificateEntryRef::X509(bytes)) => Some(*bytes),
            _ => None,
        };
        for root in &self.roots {
            let mut verifier = Box::new(RootVerifier::new(Certificate::X509(*root)));
            verifier.set_hostname_verification(&self.hostname)?;
            let root_is_presented = certificate.entries.len() > 1 && presented_root == Some(*root);
            match verifier.verify_certificate(
                transcript,
                clone_certificate(&certificate, root_is_presented)?,
            ) {
                Ok(()) => {
                    self.selected = Some(verifier);
                    self.leaf = match certificate.entries.first() {
                        Some(CertificateEntryRef::X509(bytes)) => bytes.to_vec(),
                        _ => return Err(TlsError::InvalidCertificate),
                    };
                    self.transcript = Some(transcript.clone());
                    return Ok(());
                }
                Err(error) if presented_root == Some(*root) => {
                    let _ = microsystem_user_rt::debug_write(
                        alloc::format!("[tls] presented-root validation error={error:?}\n")
                            .as_bytes(),
                    );
                }
                Err(_) => {}
            }
        }
        Err(TlsError::InvalidCertificate)
    }

    fn verify_signature(&mut self, verify: CertificateVerifyRef<'_>) -> Result<(), TlsError> {
        let result = match verify.signature_scheme {
            SignatureScheme::EcdsaSecp384r1Sha384 => verify_p384_signature(
                &self.leaf,
                self.transcript.take().ok_or(TlsError::InvalidCertificate)?,
                verify,
            ),
            SignatureScheme::RsaPssRsaeSha256
            | SignatureScheme::RsaPssRsaeSha384
            | SignatureScheme::RsaPssRsaeSha512 => verify_rsa_signature(
                &self.leaf,
                self.transcript.take().ok_or(TlsError::InvalidCertificate)?,
                verify,
            ),
            _ => self
                .selected
                .as_mut()
                .ok_or(TlsError::InvalidCertificate)?
                .verify_signature(verify),
        };
        if let Err(error) = &result {
            let _ = microsystem_user_rt::debug_write(
                alloc::format!("[tls] handshake-signature validation error={error:?}\n").as_bytes(),
            );
        }
        result
    }
}

fn verify_p384_signature(
    certificate: &[u8],
    transcript: HandshakeHash,
    verify: CertificateVerifyRef<'_>,
) -> Result<(), TlsError> {
    use p384::ecdsa::{signature::Verifier, Signature, VerifyingKey};

    let public_key = VerifyingKey::from_sec1_bytes(certificate_public_key(certificate)?)
        .map_err(|_| TlsError::DecodeError)?;
    let signature = Signature::from_der(verify.signature).map_err(|_| TlsError::DecodeError)?;
    let mut message = Vec::with_capacity(146);
    message.resize(64, 0x20);
    message.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
    message.extend_from_slice(&transcript.finalize());
    public_key
        .verify(&message, &signature)
        .map_err(|_| TlsError::InvalidSignature)
}

fn verify_rsa_signature(
    certificate: &[u8],
    transcript: HandshakeHash,
    verify: CertificateVerifyRef<'_>,
) -> Result<(), TlsError> {
    use rsa::{pkcs1::DecodeRsaPublicKey, signature::Verifier, RsaPublicKey};

    let public_key = RsaPublicKey::from_pkcs1_der(certificate_public_key(certificate)?)
        .map_err(|_| TlsError::DecodeError)?;
    let mut message = Vec::with_capacity(146);
    message.resize(64, 0x20);
    message.extend_from_slice(b"TLS 1.3, server CertificateVerify\0");
    message.extend_from_slice(&transcript.finalize());
    let verified = match verify.signature_scheme {
        SignatureScheme::RsaPssRsaeSha256 => {
            let signature = rsa::pss::Signature::try_from(verify.signature)
                .map_err(|_| TlsError::DecodeError)?;
            rsa::pss::VerifyingKey::<sha2::Sha256>::from(public_key)
                .verify(&message, &signature)
                .is_ok()
        }
        SignatureScheme::RsaPssRsaeSha384 => {
            let signature = rsa::pss::Signature::try_from(verify.signature)
                .map_err(|_| TlsError::DecodeError)?;
            rsa::pss::VerifyingKey::<sha2::Sha384>::from(public_key)
                .verify(&message, &signature)
                .is_ok()
        }
        SignatureScheme::RsaPssRsaeSha512 => {
            let signature = rsa::pss::Signature::try_from(verify.signature)
                .map_err(|_| TlsError::DecodeError)?;
            rsa::pss::VerifyingKey::<sha2::Sha512>::from(public_key)
                .verify(&message, &signature)
                .is_ok()
        }
        _ => return Err(TlsError::InvalidSignatureScheme),
    };
    verified.then_some(()).ok_or(TlsError::InvalidSignature)
}

fn certificate_public_key(certificate: &[u8]) -> Result<&[u8], TlsError> {
    let certificate_bytes = certificate.len();
    let mut outer_cursor = 0usize;
    let (tag, certificate) = der_tlv(certificate, &mut outer_cursor)?;
    if tag != 0x30 || outer_cursor != certificate_bytes {
        return Err(TlsError::DecodeError);
    }
    let mut certificate_cursor = 0usize;
    let (tag, tbs) = der_tlv(certificate, &mut certificate_cursor)?;
    if tag != 0x30 {
        return Err(TlsError::DecodeError);
    }
    let mut cursor = 0usize;
    if tbs.first() == Some(&0xa0) {
        der_tlv(tbs, &mut cursor)?;
    }
    for expected in [0x02, 0x30, 0x30, 0x30, 0x30] {
        let (tag, _) = der_tlv(tbs, &mut cursor)?;
        if tag != expected {
            return Err(TlsError::DecodeError);
        }
    }
    let (tag, subject_public_key_info) = der_tlv(tbs, &mut cursor)?;
    if tag != 0x30 {
        return Err(TlsError::DecodeError);
    }
    let mut cursor = 0usize;
    let (tag, _) = der_tlv(subject_public_key_info, &mut cursor)?;
    if tag != 0x30 {
        return Err(TlsError::DecodeError);
    }
    let (tag, bit_string) = der_tlv(subject_public_key_info, &mut cursor)?;
    if tag != 0x03 || bit_string.first() != Some(&0) {
        return Err(TlsError::DecodeError);
    }
    Ok(&bit_string[1..])
}

fn der_tlv<'a>(input: &'a [u8], cursor: &mut usize) -> Result<(u8, &'a [u8]), TlsError> {
    let tag = *input.get(*cursor).ok_or(TlsError::DecodeError)?;
    *cursor += 1;
    let first = *input.get(*cursor).ok_or(TlsError::DecodeError)?;
    *cursor += 1;
    let length = if first & 0x80 == 0 {
        first as usize
    } else {
        let bytes = (first & 0x7f) as usize;
        if bytes == 0 || bytes > core::mem::size_of::<usize>() {
            return Err(TlsError::DecodeError);
        }
        let mut length = 0usize;
        for _ in 0..bytes {
            length = length
                .checked_mul(256)
                .and_then(|value| input.get(*cursor).map(|byte| value + *byte as usize))
                .ok_or(TlsError::DecodeError)?;
            *cursor += 1;
        }
        length
    };
    let end = (*cursor).checked_add(length).ok_or(TlsError::DecodeError)?;
    let value = input.get(*cursor..end).ok_or(TlsError::DecodeError)?;
    *cursor = end;
    Ok((tag, value))
}

fn clone_certificate<'a>(
    certificate: &CertificateRef<'a>,
    omit_presented_root: bool,
) -> Result<CertificateRef<'a>, TlsError> {
    let mut cloned = CertificateRef::with_context(&[]);
    let entries = if omit_presented_root {
        certificate
            .entries
            .get(..certificate.entries.len().saturating_sub(1))
            .ok_or(TlsError::InvalidCertificate)?
    } else {
        &certificate.entries
    };
    for entry in entries {
        cloned.add(match entry {
            CertificateEntryRef::X509(bytes) => CertificateEntryRef::X509(bytes),
            CertificateEntryRef::RawPublicKey(bytes) => CertificateEntryRef::RawPublicKey(bytes),
        })?;
    }
    Ok(cloned)
}

struct VerifiedProvider<'a> {
    rng: KernelRng,
    verifier: MozillaVerifier<'a>,
}

impl CryptoProvider for VerifiedProvider<'_> {
    type CipherSuite = Aes128GcmSha256;
    type Signature = Vec<u8>;

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Aes128GcmSha256>, TlsError> {
        Ok(&mut self.verifier)
    }
}
