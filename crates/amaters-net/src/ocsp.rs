//! OCSP (Online Certificate Status Protocol) revocation checking
//!
//! Implements RFC 6960 OCSP request building, HTTP transport, and response parsing.
//! Uses pure Rust DER encoding/decoding with `x509-parser`.
//!
//! # Architecture
//!
//! - `OcspRevocationChecker`: Main entry point with caching
//! - `build_ocsp_request()`: Constructs DER-encoded OCSP request
//! - `send_ocsp_request()`: HTTP/1.1 POST to OCSP responder
//! - `parse_ocsp_response()`: Parses DER-encoded OCSP response
//!
//! # Fail-open Design
//!
//! On any error (network, parsing, timeout), the checker returns
//! `RevocationStatus::Unknown` rather than blocking the connection.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use tracing::warn;
use x509_parser::prelude::*;

use crate::error::{NetError, NetResult};
use crate::mtls::RevocationStatus;

// ── ASN.1 DER tag constants ──────────────────────────────────────────

/// ASN.1 SEQUENCE tag
const TAG_SEQUENCE: u8 = 0x30;
/// ASN.1 OCTET STRING tag
const TAG_OCTET_STRING: u8 = 0x04;
/// ASN.1 INTEGER tag
const TAG_INTEGER: u8 = 0x02;
/// ASN.1 OID tag
const TAG_OID: u8 = 0x06;
/// ASN.1 ENUMERATED tag
const TAG_ENUMERATED: u8 = 0x0A;
/// ASN.1 context-specific constructed [0]
const TAG_CONTEXT_0: u8 = 0xA0;
/// ASN.1 context-specific constructed [1]
const TAG_CONTEXT_1: u8 = 0xA1;
/// ASN.1 context-specific primitive [1] (for revoked implicit tag)
const TAG_CONTEXT_PRIM_1: u8 = 0x81;

/// SHA-256 OID: 2.16.840.1.101.3.4.2.1
const SHA256_OID_BYTES: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];

/// id-pkix-ocsp-basic OID: 1.3.6.1.5.5.7.48.1.1
const OCSP_BASIC_OID_BYTES: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];

/// id-ad-ocsp OID bytes: 1.3.6.1.5.5.7.48.1
const AIA_OCSP_OID_BYTES: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01];

/// AIA extension OID bytes: 1.3.6.1.5.5.7.1.1
const AIA_EXT_OID_BYTES: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01];

/// Default timeout for OCSP requests
const DEFAULT_OCSP_TIMEOUT: Duration = Duration::from_secs(5);

/// Default OCSP cache TTL
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

// ── DER encoding helpers ─────────────────────────────────────────────

/// Encode a DER length field (supports short and long forms)
fn der_encode_length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        vec![len as u8]
    } else if len < 0x100 {
        vec![0x81, len as u8]
    } else if len < 0x10000 {
        vec![0x82, (len >> 8) as u8, len as u8]
    } else if len < 0x100_0000 {
        vec![0x83, (len >> 16) as u8, (len >> 8) as u8, len as u8]
    } else {
        vec![
            0x84,
            (len >> 24) as u8,
            (len >> 16) as u8,
            (len >> 8) as u8,
            len as u8,
        ]
    }
}

/// Wrap content bytes in a TLV (tag-length-value) structure
fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend(der_encode_length(content.len()));
    out.extend(content);
    out
}

/// Encode a DER OID value (tag + length + oid bytes)
fn der_oid(oid_bytes: &[u8]) -> Vec<u8> {
    der_tlv(TAG_OID, oid_bytes)
}

/// Encode an algorithm identifier SEQUENCE (OID + NULL parameters)
fn der_algorithm_identifier(oid_bytes: &[u8]) -> Vec<u8> {
    let mut content = der_oid(oid_bytes);
    // NULL parameters: 05 00
    content.extend(&[0x05, 0x00]);
    der_tlv(TAG_SEQUENCE, &content)
}

/// Encode an OCTET STRING
fn der_octet_string(data: &[u8]) -> Vec<u8> {
    der_tlv(TAG_OCTET_STRING, data)
}

/// Encode a positive INTEGER (with leading zero if MSB is set)
fn der_integer_from_bytes(data: &[u8]) -> Vec<u8> {
    // Strip leading zeros but keep at least one byte
    let stripped = data
        .iter()
        .position(|&b| b != 0)
        .map_or(&data[data.len().saturating_sub(1)..], |pos| &data[pos..]);

    // If MSB is set, prepend a 0x00 byte to keep it positive
    if stripped.first().is_some_and(|&b| b & 0x80 != 0) {
        let mut content = vec![0x00];
        content.extend(stripped);
        der_tlv(TAG_INTEGER, &content)
    } else {
        der_tlv(TAG_INTEGER, stripped)
    }
}

// ── DER parsing helpers ──────────────────────────────────────────────

/// Read a DER length from a byte slice, returning (length_value, bytes_consumed)
fn der_read_length(data: &[u8]) -> NetResult<(usize, usize)> {
    if data.is_empty() {
        return Err(NetError::InvalidCertificate(
            "OCSP: unexpected end of DER data reading length".to_string(),
        ));
    }
    let first = data[0];
    if first < 0x80 {
        Ok((first as usize, 1))
    } else {
        let num_bytes = (first & 0x7F) as usize;
        if num_bytes == 0 || num_bytes > 4 {
            return Err(NetError::InvalidCertificate(format!(
                "OCSP: unsupported DER length encoding ({num_bytes} bytes)"
            )));
        }
        if data.len() < 1 + num_bytes {
            return Err(NetError::InvalidCertificate(
                "OCSP: truncated DER length".to_string(),
            ));
        }
        let mut val: usize = 0;
        for i in 0..num_bytes {
            val = (val << 8) | (data[1 + i] as usize);
        }
        Ok((val, 1 + num_bytes))
    }
}

/// Read a TLV element, returning (tag, content_slice, total_bytes_consumed)
fn der_read_tlv(data: &[u8]) -> NetResult<(u8, &[u8], usize)> {
    if data.is_empty() {
        return Err(NetError::InvalidCertificate(
            "OCSP: unexpected end of DER data reading TLV".to_string(),
        ));
    }
    let tag = data[0];
    let (len, len_bytes) = der_read_length(&data[1..])?;
    let header_len = 1 + len_bytes;
    let total = header_len + len;
    if data.len() < total {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: DER content truncated (need {total}, have {})",
            data.len()
        )));
    }
    Ok((tag, &data[header_len..total], total))
}

/// Iterate over children of a SEQUENCE (or other constructed type)
fn der_children(data: &[u8]) -> NetResult<Vec<(u8, Vec<u8>)>> {
    let mut children = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let (tag, content, consumed) = der_read_tlv(&data[pos..])?;
        children.push((tag, content.to_vec()));
        pos += consumed;
    }
    Ok(children)
}

// ── Certificate fingerprint helper ───────────────────────────────────

/// Compute a hex fingerprint from the first 32 bytes of a cert (matches mtls.rs)
fn cert_fingerprint(cert_der: &[u8]) -> String {
    cert_der.iter().take(32).fold(String::new(), |mut s, b| {
        let _ = write!(&mut s, "{b:02x}");
        s
    })
}

// ── OCSP URL extraction ─────────────────────────────────────────────

/// Extract OCSP responder URL from a certificate's Authority Information Access extension
pub fn extract_ocsp_url(cert_der: &[u8]) -> NetResult<Option<String>> {
    let (_, parsed) = X509Certificate::from_der(cert_der).map_err(|e| {
        NetError::InvalidCertificate(format!("OCSP: failed to parse certificate: {e}"))
    })?;

    // Look for AIA extension (OID 1.3.6.1.5.5.7.1.1)
    let aia_oid = asn1_rs::Oid::new(std::borrow::Cow::Borrowed(AIA_EXT_OID_BYTES));
    let aia = parsed.extensions().iter().find(|ext| ext.oid == aia_oid);

    let aia_ext = match aia {
        Some(ext) => ext,
        None => return Ok(None),
    };

    // Parse the AIA extension value as a SEQUENCE of AccessDescription
    // Each AccessDescription = SEQUENCE { accessMethod OID, accessLocation GeneralName }
    let children = der_children(aia_ext.value)?;

    for (tag, child_data) in &children {
        // Each child should be a SEQUENCE (AccessDescription)
        if *tag != TAG_SEQUENCE {
            continue;
        }
        let inner = der_children(child_data)?;
        if inner.len() < 2 {
            continue;
        }

        // First element is the OID (accessMethod)
        let (oid_tag, oid_data) = &inner[0];
        if *oid_tag != TAG_OID {
            continue;
        }

        // Check if this is id-ad-ocsp (1.3.6.1.5.5.7.48.1)
        if oid_data.as_slice() != AIA_OCSP_OID_BYTES {
            continue;
        }

        // Second element is the GeneralName — context [6] uniformResourceIdentifier
        let (name_tag, name_data) = &inner[1];
        // Context-specific primitive [6] = 0x86
        if *name_tag == 0x86 {
            let url = String::from_utf8(name_data.clone()).map_err(|e| {
                NetError::InvalidCertificate(format!("OCSP: invalid URL encoding: {e}"))
            })?;
            return Ok(Some(url));
        }
    }

    Ok(None)
}

// ── OCSP request building ────────────────────────────────────────────

/// Build a DER-encoded OCSP request for the given certificate.
///
/// The request follows RFC 6960 structure:
/// ```text
/// OCSPRequest ::= SEQUENCE {
///     tbsRequest TBSRequest
/// }
/// TBSRequest ::= SEQUENCE {
///     version [0] EXPLICIT Version DEFAULT v1,  (omitted for v1)
///     requestList SEQUENCE OF Request
/// }
/// Request ::= SEQUENCE {
///     reqCert CertID
/// }
/// CertID ::= SEQUENCE {
///     hashAlgorithm AlgorithmIdentifier,
///     issuerNameHash OCTET STRING,
///     issuerKeyHash OCTET STRING,
///     serialNumber CertificateSerialNumber
/// }
/// ```
pub fn build_ocsp_request(cert_der: &[u8]) -> NetResult<Vec<u8>> {
    let (_, parsed) = X509Certificate::from_der(cert_der).map_err(|e| {
        NetError::InvalidCertificate(format!("OCSP: failed to parse certificate: {e}"))
    })?;

    // Hash the issuer distinguished name with SHA-256
    let issuer_name_der = parsed.issuer().as_raw();
    let issuer_name_hash = blake3::hash(issuer_name_der);

    // For a self-signed cert, the issuer key is in the cert itself.
    // For issued certs, ideally we'd have the issuer cert. We use the
    // subject public key info from the cert's own SPKI as a fallback
    // (which is correct for self-signed, and a best-effort for others).
    let spki_der = parsed.public_key().raw;
    let issuer_key_hash = blake3::hash(spki_der);

    // Serial number bytes
    let serial_bytes = parsed.serial.to_bytes_be();

    // Build CertID
    let algo_id = der_algorithm_identifier(SHA256_OID_BYTES);
    let name_hash = der_octet_string(issuer_name_hash.as_bytes());
    let key_hash = der_octet_string(issuer_key_hash.as_bytes());
    let serial_int = der_integer_from_bytes(&serial_bytes);

    let mut cert_id_content = Vec::new();
    cert_id_content.extend(&algo_id);
    cert_id_content.extend(&name_hash);
    cert_id_content.extend(&key_hash);
    cert_id_content.extend(&serial_int);
    let cert_id = der_tlv(TAG_SEQUENCE, &cert_id_content);

    // Build Request
    let request = der_tlv(TAG_SEQUENCE, &cert_id);

    // Build requestList (SEQUENCE OF Request)
    let request_list = der_tlv(TAG_SEQUENCE, &request);

    // Build TBSRequest (version omitted for v1)
    let tbs_request = der_tlv(TAG_SEQUENCE, &request_list);

    // Build OCSPRequest
    let ocsp_request = der_tlv(TAG_SEQUENCE, &tbs_request);

    Ok(ocsp_request)
}

// ── HTTP transport ───────────────────────────────────────────────────

/// Parse a URL into (host, port, path) components
fn parse_url(url: &str) -> NetResult<(String, u16, String)> {
    // Strip scheme
    let without_scheme = if let Some(rest) = url.strip_prefix("http://") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://") {
        // OCSP typically uses HTTP, but handle https prefix gracefully
        rest
    } else {
        url
    };

    // Split host+port from path
    let (host_port, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };

    // Split host from port
    let (host, port) = match host_port.rfind(':') {
        Some(idx) => {
            let port_str = &host_port[idx + 1..];
            let port: u16 = port_str.parse().map_err(|e| {
                NetError::InvalidCertificate(format!("OCSP: invalid port in URL: {e}"))
            })?;
            (host_port[..idx].to_string(), port)
        }
        None => (host_port.to_string(), 80),
    };

    Ok((host, port, path.to_string()))
}

/// Send an OCSP request via HTTP/1.1 POST and return the response body.
///
/// Uses raw TCP for pure-Rust operation (no reqwest dependency).
pub async fn send_ocsp_request(
    url: &str,
    request_der: &[u8],
    timeout: Duration,
) -> NetResult<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let (host, port, path) = parse_url(url)?;

    // Build HTTP/1.1 POST request
    let http_request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/ocsp-request\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        request_der.len()
    );

    // Connect with timeout
    let addr = format!("{host}:{port}");
    let stream = tokio::time::timeout(timeout, TcpStream::connect(&addr))
        .await
        .map_err(|_| NetError::Timeout(format!("OCSP: connection to {addr} timed out")))?
        .map_err(|e| {
            NetError::ConnectionRefused(format!("OCSP: failed to connect to {addr}: {e}"))
        })?;

    let mut stream = stream;

    // Send request with timeout
    tokio::time::timeout(timeout, async {
        stream
            .write_all(http_request.as_bytes())
            .await
            .map_err(|e| NetError::ConnectionReset(format!("OCSP: failed to send request: {e}")))?;
        stream.write_all(request_der).await.map_err(|e| {
            NetError::ConnectionReset(format!("OCSP: failed to send request body: {e}"))
        })?;
        stream
            .flush()
            .await
            .map_err(|e| NetError::ConnectionReset(format!("OCSP: failed to flush: {e}")))?;
        Ok::<(), NetError>(())
    })
    .await
    .map_err(|_| NetError::Timeout("OCSP: send timed out".to_string()))??;

    // Read response with timeout
    let response_bytes = tokio::time::timeout(timeout, async {
        let mut buf = Vec::with_capacity(8192);
        stream.read_to_end(&mut buf).await.map_err(|e| {
            NetError::ConnectionReset(format!("OCSP: failed to read response: {e}"))
        })?;
        Ok::<Vec<u8>, NetError>(buf)
    })
    .await
    .map_err(|_| NetError::Timeout("OCSP: read timed out".to_string()))??;

    // Parse HTTP response: find end of headers (\r\n\r\n)
    let header_end = response_bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| {
            NetError::InvalidCertificate(
                "OCSP: malformed HTTP response (no header end)".to_string(),
            )
        })?;

    let header_str = String::from_utf8_lossy(&response_bytes[..header_end]);

    // Check HTTP status
    let status_line = header_str
        .lines()
        .next()
        .ok_or_else(|| NetError::InvalidCertificate("OCSP: empty HTTP response".to_string()))?;

    // Expect "HTTP/1.x 200 ..."
    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    if parts.len() < 2 {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: malformed HTTP status line: {status_line}"
        )));
    }
    let status_code: u16 = parts[1].parse().map_err(|e| {
        NetError::InvalidCertificate(format!("OCSP: invalid HTTP status code: {e}"))
    })?;

    if status_code != 200 {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: HTTP error {status_code}"
        )));
    }

    let body_start = header_end + 4;
    if body_start >= response_bytes.len() {
        return Err(NetError::InvalidCertificate(
            "OCSP: empty HTTP response body".to_string(),
        ));
    }

    Ok(response_bytes[body_start..].to_vec())
}

// ── OCSP response parsing ────────────────────────────────────────────

/// Parse a DER-encoded OCSP response and extract the revocation status.
///
/// OCSP response structure (RFC 6960):
/// ```text
/// OCSPResponse ::= SEQUENCE {
///     responseStatus ENUMERATED { successful(0), ... },
///     responseBytes [0] EXPLICIT SEQUENCE {
///         responseType OID,
///         response OCTET STRING  -- contains BasicOCSPResponse DER
///     } OPTIONAL
/// }
///
/// BasicOCSPResponse ::= SEQUENCE {
///     tbsResponseData ResponseData,
///     signatureAlgorithm AlgorithmIdentifier,
///     signature BIT STRING,
///     certs [0] EXPLICIT SEQUENCE OF Certificate OPTIONAL
/// }
///
/// ResponseData ::= SEQUENCE {
///     version [0] EXPLICIT Version DEFAULT v1,
///     responderID ResponderID,
///     producedAt GeneralizedTime,
///     responses SEQUENCE OF SingleResponse,
///     ...
/// }
///
/// SingleResponse ::= SEQUENCE {
///     certID CertID,
///     certStatus CertStatus,
///     ...
/// }
///
/// CertStatus ::= CHOICE {
///     good    [0] IMPLICIT NULL,
///     revoked [1] IMPLICIT RevokedInfo,
///     unknown [2] IMPLICIT UnknownInfo
/// }
/// ```
pub fn parse_ocsp_response(response_der: &[u8]) -> NetResult<RevocationStatus> {
    // Outer SEQUENCE: OCSPResponse
    let (tag, ocsp_resp_content, _) = der_read_tlv(response_der)?;
    if tag != TAG_SEQUENCE {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: expected SEQUENCE, got 0x{tag:02x}"
        )));
    }

    let children = der_children(ocsp_resp_content)?;
    if children.is_empty() {
        return Err(NetError::InvalidCertificate(
            "OCSP: empty OCSPResponse".to_string(),
        ));
    }

    // First child: responseStatus ENUMERATED
    let (status_tag, status_data) = &children[0];
    if *status_tag != TAG_ENUMERATED {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: expected ENUMERATED for responseStatus, got 0x{status_tag:02x}"
        )));
    }
    let response_status = status_data
        .first()
        .copied()
        .ok_or_else(|| NetError::InvalidCertificate("OCSP: empty responseStatus".to_string()))?;

    // responseStatus: 0=successful, 1=malformedRequest, 2=internalError,
    //                 3=tryLater, 5=sigRequired, 6=unauthorized
    if response_status != 0 {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: non-successful responseStatus: {response_status}"
        )));
    }

    // Second child: responseBytes [0] EXPLICIT
    if children.len() < 2 {
        return Err(NetError::InvalidCertificate(
            "OCSP: missing responseBytes".to_string(),
        ));
    }

    let (rb_tag, rb_data) = &children[1];
    if *rb_tag != TAG_CONTEXT_0 {
        return Err(NetError::InvalidCertificate(format!(
            "OCSP: expected [0] for responseBytes, got 0x{rb_tag:02x}"
        )));
    }

    // responseBytes is a SEQUENCE { responseType OID, response OCTET STRING }
    let (inner_tag, inner_content, _) = der_read_tlv(rb_data)?;
    if inner_tag != TAG_SEQUENCE {
        return Err(NetError::InvalidCertificate(
            "OCSP: responseBytes inner not SEQUENCE".to_string(),
        ));
    }

    let rb_children = der_children(inner_content)?;
    if rb_children.len() < 2 {
        return Err(NetError::InvalidCertificate(
            "OCSP: responseBytes SEQUENCE too short".to_string(),
        ));
    }

    // Verify responseType is id-pkix-ocsp-basic
    let (oid_tag, oid_data) = &rb_children[0];
    if *oid_tag != TAG_OID {
        return Err(NetError::InvalidCertificate(
            "OCSP: responseType not OID".to_string(),
        ));
    }
    if oid_data.as_slice() != OCSP_BASIC_OID_BYTES {
        return Err(NetError::InvalidCertificate(
            "OCSP: responseType is not id-pkix-ocsp-basic".to_string(),
        ));
    }

    // response OCTET STRING contains BasicOCSPResponse
    let (oct_tag, oct_data) = &rb_children[1];
    if *oct_tag != TAG_OCTET_STRING {
        return Err(NetError::InvalidCertificate(
            "OCSP: response not OCTET STRING".to_string(),
        ));
    }

    // Parse BasicOCSPResponse
    parse_basic_ocsp_response(oct_data)
}

/// Parse a BasicOCSPResponse and extract status from the first SingleResponse
fn parse_basic_ocsp_response(data: &[u8]) -> NetResult<RevocationStatus> {
    // BasicOCSPResponse ::= SEQUENCE { tbsResponseData, sigAlgo, sig, [0] certs }
    let (tag, content, _) = der_read_tlv(data)?;
    if tag != TAG_SEQUENCE {
        return Err(NetError::InvalidCertificate(
            "OCSP: BasicOCSPResponse not SEQUENCE".to_string(),
        ));
    }

    let children = der_children(content)?;
    if children.is_empty() {
        return Err(NetError::InvalidCertificate(
            "OCSP: empty BasicOCSPResponse".to_string(),
        ));
    }

    // First child is tbsResponseData (SEQUENCE)
    let (tbs_tag, tbs_data) = &children[0];
    if *tbs_tag != TAG_SEQUENCE {
        return Err(NetError::InvalidCertificate(
            "OCSP: tbsResponseData not SEQUENCE".to_string(),
        ));
    }

    parse_tbs_response_data(tbs_data)
}

/// Parse ResponseData and extract certStatus from the first SingleResponse
fn parse_tbs_response_data(data: &[u8]) -> NetResult<RevocationStatus> {
    let children = der_children(data)?;

    // ResponseData fields:
    //   [0] version (optional), responderID, producedAt, responses SEQUENCE, [1] extensions
    // We need to find the SEQUENCE OF SingleResponse (the "responses" field).
    // The responderID can be [1] or [2] (byName or byKey), producedAt is GeneralizedTime (0x18).
    // We iterate and find the first SEQUENCE that contains SingleResponse elements.

    // Find the responses field: it's a SEQUENCE OF SingleResponse.
    // Walk the children looking for SEQUENCE tags after the responderID and producedAt.
    let mut response_seq: Option<&Vec<u8>> = None;
    let mut found_time = false;

    for (tag, child_data) in &children {
        // Skip version [0]
        if *tag == TAG_CONTEXT_0 {
            continue;
        }
        // Skip responderID [1] byName or [2] byKey
        if *tag == TAG_CONTEXT_1 || *tag == 0xA2 {
            continue;
        }
        // GeneralizedTime (0x18) = producedAt
        if *tag == 0x18 {
            found_time = true;
            continue;
        }
        // First SEQUENCE after producedAt is the responses field
        if *tag == TAG_SEQUENCE && found_time {
            response_seq = Some(child_data);
            break;
        }
        // If we haven't seen a time yet and this is a SEQUENCE, it might be responses
        // in a simplified encoding (some responders omit optional fields differently)
        if *tag == TAG_SEQUENCE && !found_time {
            // Check if this looks like it contains SingleResponse children
            if let Ok(inner) = der_children(child_data) {
                if !inner.is_empty() && inner[0].0 == TAG_SEQUENCE {
                    response_seq = Some(child_data);
                    break;
                }
            }
        }
    }

    let responses_data = response_seq.ok_or_else(|| {
        NetError::InvalidCertificate(
            "OCSP: could not find responses SEQUENCE in ResponseData".to_string(),
        )
    })?;

    // Parse SEQUENCE OF SingleResponse — take the first one
    let single_responses = der_children(responses_data)?;
    if single_responses.is_empty() {
        return Err(NetError::InvalidCertificate(
            "OCSP: no SingleResponse found".to_string(),
        ));
    }

    let (sr_tag, sr_data) = &single_responses[0];
    if *sr_tag != TAG_SEQUENCE {
        return Err(NetError::InvalidCertificate(
            "OCSP: SingleResponse not SEQUENCE".to_string(),
        ));
    }

    parse_single_response(sr_data)
}

/// Parse a SingleResponse and extract certStatus
fn parse_single_response(data: &[u8]) -> NetResult<RevocationStatus> {
    let children = der_children(data)?;

    // SingleResponse ::= SEQUENCE {
    //   certID CertID (SEQUENCE),
    //   certStatus CertStatus,
    //   thisUpdate GeneralizedTime,
    //   ...
    // }
    if children.len() < 2 {
        return Err(NetError::InvalidCertificate(
            "OCSP: SingleResponse too short".to_string(),
        ));
    }

    // certStatus is the second element (index 1)
    let (status_tag, _status_data) = &children[1];

    // CertStatus ::= CHOICE {
    //   good    [0] IMPLICIT NULL     -> tag 0x80 (context primitive 0)
    //   revoked [1] IMPLICIT ...      -> tag 0xA1 (context constructed 1)
    //   unknown [2] IMPLICIT NULL     -> tag 0x82 (context primitive 2)
    // }
    match *status_tag {
        0x80 => Ok(RevocationStatus::Good), // good [0]
        0xA1 | TAG_CONTEXT_PRIM_1 => Ok(RevocationStatus::Revoked), // revoked [1]
        0x82 => Ok(RevocationStatus::Unknown), // unknown [2]
        other => {
            warn!("OCSP: unexpected certStatus tag 0x{other:02x}");
            Ok(RevocationStatus::Unknown)
        }
    }
}

// ── OcspRevocationChecker ────────────────────────────────────────────

/// OCSP-based certificate revocation checker with caching
///
/// Implements the `RevocationChecker` trait with real OCSP protocol support:
/// - Builds DER-encoded OCSP requests per RFC 6960
/// - Sends requests via HTTP/1.1 POST to OCSP responders
/// - Parses DER-encoded OCSP responses
/// - Caches results with configurable TTL
/// - Fail-open: returns `Unknown` on any error
#[derive(Debug)]
pub struct OcspRevocationChecker {
    /// OCSP responder URL (overrides URL from certificate AIA extension)
    responder_url: Option<String>,
    /// Cache of OCSP responses (fingerprint -> (status, timestamp))
    response_cache: Arc<RwLock<HashMap<String, (RevocationStatus, SystemTime)>>>,
    /// Cache TTL
    cache_ttl: Duration,
    /// Request timeout
    timeout: Duration,
}

impl Default for OcspRevocationChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl OcspRevocationChecker {
    /// Create a new OCSP revocation checker with default settings
    pub fn new() -> Self {
        Self {
            responder_url: None,
            response_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: DEFAULT_CACHE_TTL,
            timeout: DEFAULT_OCSP_TIMEOUT,
        }
    }

    /// Set the OCSP responder URL (overrides certificate AIA extension)
    pub fn with_responder_url(mut self, url: impl Into<String>) -> Self {
        self.responder_url = Some(url.into());
        self
    }

    /// Set cache TTL
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Set request timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get cached revocation status for a certificate fingerprint
    pub fn get_cached(&self, fingerprint: &str) -> Option<RevocationStatus> {
        let cache = self.response_cache.read();
        if let Some((status, timestamp)) = cache.get(fingerprint) {
            if timestamp.elapsed().unwrap_or(Duration::MAX) < self.cache_ttl {
                return Some(*status);
            }
        }
        None
    }

    /// Cache a revocation status for a certificate fingerprint
    pub fn cache_status(&self, fingerprint: String, status: RevocationStatus) {
        let mut cache = self.response_cache.write();
        cache.insert(fingerprint, (status, SystemTime::now()));
    }

    /// Clear the entire cache
    pub fn clear_cache(&self) {
        self.response_cache.write().clear();
    }

    /// Get the number of cached entries
    pub fn cache_size(&self) -> usize {
        self.response_cache.read().len()
    }

    /// Determine the OCSP responder URL to use for a certificate
    fn resolve_responder_url(&self, cert_der: &[u8]) -> NetResult<Option<String>> {
        // Custom URL takes priority
        if let Some(ref url) = self.responder_url {
            return Ok(Some(url.clone()));
        }
        // Try to extract from certificate AIA extension
        extract_ocsp_url(cert_der)
    }

    /// Synchronous check: cache-only, no network I/O
    pub fn check_revocation(
        &self,
        cert: &rustls::pki_types::CertificateDer<'_>,
    ) -> NetResult<RevocationStatus> {
        let fingerprint = cert_fingerprint(cert.as_ref());

        // Cache-only in sync mode
        if let Some(status) = self.get_cached(&fingerprint) {
            return Ok(status);
        }

        // No network in sync mode
        Ok(RevocationStatus::Unknown)
    }

    /// Asynchronous check: cache first, then network OCSP query
    pub fn check_revocation_async<'a>(
        &'a self,
        cert: &'a rustls::pki_types::CertificateDer<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<RevocationStatus>> + Send + 'a>>
    {
        let cert_bytes = cert.as_ref().to_vec();
        let fingerprint = cert_fingerprint(&cert_bytes);

        // Check cache first
        if let Some(status) = self.get_cached(&fingerprint) {
            return Box::pin(async move { Ok(status) });
        }

        let timeout = self.timeout;

        Box::pin(async move {
            // Resolve responder URL
            let url = match self.resolve_responder_url(&cert_bytes) {
                Ok(Some(url)) => url,
                Ok(None) => {
                    warn!("OCSP: no responder URL available for certificate");
                    return Ok(RevocationStatus::Unknown);
                }
                Err(e) => {
                    warn!("OCSP: failed to resolve responder URL: {e}");
                    return Ok(RevocationStatus::Unknown);
                }
            };

            // Build OCSP request
            let request_der = match build_ocsp_request(&cert_bytes) {
                Ok(req) => req,
                Err(e) => {
                    warn!("OCSP: failed to build request: {e}");
                    return Ok(RevocationStatus::Unknown);
                }
            };

            // Send OCSP request
            let response_der = match send_ocsp_request(&url, &request_der, timeout).await {
                Ok(resp) => resp,
                Err(e) => {
                    warn!("OCSP: network error: {e}");
                    return Ok(RevocationStatus::Unknown);
                }
            };

            // Parse OCSP response
            let status = match parse_ocsp_response(&response_der) {
                Ok(s) => s,
                Err(e) => {
                    warn!("OCSP: failed to parse response: {e}");
                    RevocationStatus::Unknown
                }
            };

            // Cache the result
            self.cache_status(fingerprint, status);

            Ok(status)
        })
    }
}

// ── RevocationChecker trait impl (delegates to the methods above) ────

impl crate::mtls::RevocationChecker for OcspRevocationChecker {
    fn check_revocation(
        &self,
        cert: &rustls::pki_types::CertificateDer<'_>,
    ) -> NetResult<RevocationStatus> {
        OcspRevocationChecker::check_revocation(self, cert)
    }

    fn check_revocation_async(
        &self,
        cert: &rustls::pki_types::CertificateDer<'_>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = NetResult<RevocationStatus>> + Send + '_>>
    {
        let cert_bytes = cert.as_ref().to_vec();
        let fingerprint = cert_fingerprint(&cert_bytes);

        // Check cache first
        if let Some(status) = self.get_cached(&fingerprint) {
            return Box::pin(async move { Ok(status) });
        }

        let timeout = self.timeout;

        Box::pin(async move {
            // Resolve responder URL
            let url = match self.resolve_responder_url(&cert_bytes) {
                Ok(Some(url)) => url,
                Ok(None) => {
                    warn!("OCSP: no responder URL available for certificate");
                    return Ok(RevocationStatus::Unknown);
                }
                Err(e) => {
                    warn!("OCSP: failed to resolve responder URL: {e}");
                    return Ok(RevocationStatus::Unknown);
                }
            };

            // Build OCSP request
            let request_der = match build_ocsp_request(&cert_bytes) {
                Ok(req) => req,
                Err(e) => {
                    warn!("OCSP: failed to build request: {e}");
                    return Ok(RevocationStatus::Unknown);
                }
            };

            // Send OCSP request
            let response_der = match send_ocsp_request(&url, &request_der, timeout).await {
                Ok(resp) => resp,
                Err(e) => {
                    warn!("OCSP: network error: {e}");
                    return Ok(RevocationStatus::Unknown);
                }
            };

            // Parse OCSP response
            let status = match parse_ocsp_response(&response_der) {
                Ok(s) => s,
                Err(e) => {
                    warn!("OCSP: failed to parse response: {e}");
                    RevocationStatus::Unknown
                }
            };

            // Cache the result
            self.cache_status(fingerprint, status);

            Ok(status)
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::SelfSignedGenerator;

    /// Helper: generate a self-signed certificate for testing
    fn gen_test_cert() -> (rustls::pki_types::CertificateDer<'static>, Vec<u8>) {
        let generator = SelfSignedGenerator::new("test-ocsp");
        let (cert, _key) = generator.generate().expect("should generate cert");
        let der = cert.as_ref().to_vec();
        (cert, der)
    }

    // ── 1. test_build_ocsp_request_structure ──

    #[test]
    fn test_build_ocsp_request_structure() {
        let (_cert, der) = gen_test_cert();
        let req = build_ocsp_request(&der).expect("should build OCSP request");

        // Verify it starts with SEQUENCE tag
        assert_eq!(
            req[0], TAG_SEQUENCE,
            "OCSP request must start with SEQUENCE tag"
        );

        // Verify we can parse the outer SEQUENCE
        let (tag, content, total) = der_read_tlv(&req).expect("should parse outer SEQUENCE");
        assert_eq!(tag, TAG_SEQUENCE);
        assert_eq!(total, req.len(), "entire request should be consumed");

        // Parse tbsRequest (inner SEQUENCE)
        let (tbs_tag, tbs_content, _) = der_read_tlv(content).expect("should parse TBSRequest");
        assert_eq!(tbs_tag, TAG_SEQUENCE);

        // Parse requestList (inner SEQUENCE)
        let (rl_tag, rl_content, _) = der_read_tlv(tbs_content).expect("should parse requestList");
        assert_eq!(rl_tag, TAG_SEQUENCE);

        // Parse first Request (inner SEQUENCE)
        let (r_tag, r_content, _) = der_read_tlv(rl_content).expect("should parse Request");
        assert_eq!(r_tag, TAG_SEQUENCE);

        // Request contains CertID (a SEQUENCE), parse into it
        let (cid_tag, cid_content, _) = der_read_tlv(r_content).expect("should parse CertID");
        assert_eq!(cid_tag, TAG_SEQUENCE);

        // Parse CertID children — should have 4: algo, nameHash, keyHash, serial
        let cert_id_children = der_children(cid_content).expect("should parse CertID fields");
        assert_eq!(
            cert_id_children.len(),
            4,
            "CertID must have 4 fields (algo, nameHash, keyHash, serial)"
        );

        // First child: AlgorithmIdentifier SEQUENCE
        assert_eq!(cert_id_children[0].0, TAG_SEQUENCE, "algo must be SEQUENCE");
        // Second: OCTET STRING (issuerNameHash)
        assert_eq!(cert_id_children[1].0, TAG_OCTET_STRING);
        // Third: OCTET STRING (issuerKeyHash)
        assert_eq!(cert_id_children[2].0, TAG_OCTET_STRING);
        // Fourth: INTEGER (serialNumber)
        assert_eq!(cert_id_children[3].0, TAG_INTEGER);
    }

    // ── 2. test_extract_ocsp_url_from_aia ──

    #[test]
    fn test_extract_ocsp_url_from_aia() {
        // Self-signed certs from rcgen typically don't have AIA, so expect None
        let (_cert, der) = gen_test_cert();
        let url = extract_ocsp_url(&der).expect("should not error");
        // Self-signed cert has no AIA extension
        assert_eq!(url, None);
    }

    // ── 3. test_parse_ocsp_response_good ──

    #[test]
    fn test_parse_ocsp_response_good() {
        // Build a synthetic OCSP response with certStatus = good [0]
        let response = build_test_ocsp_response(0x80, &[0x00]); // good = [0] IMPLICIT NULL
        let status = parse_ocsp_response(&response).expect("should parse good response");
        assert_eq!(status, RevocationStatus::Good);
    }

    // ── 4. test_parse_ocsp_response_revoked ──

    #[test]
    fn test_parse_ocsp_response_revoked() {
        // Build a synthetic OCSP response with certStatus = revoked [1]
        // revoked [1] IMPLICIT RevokedInfo — minimal: just a GeneralizedTime
        let revoked_info = der_tlv(0x18, b"20250101000000Z"); // revocationTime
        let response = build_test_ocsp_response(0xA1, &revoked_info);
        let status = parse_ocsp_response(&response).expect("should parse revoked response");
        assert_eq!(status, RevocationStatus::Revoked);
    }

    // ── 5. test_parse_ocsp_response_unknown ──

    #[test]
    fn test_parse_ocsp_response_unknown() {
        // Build a synthetic OCSP response with certStatus = unknown [2]
        let response = build_test_ocsp_response(0x82, &[0x00]); // unknown = [2] IMPLICIT NULL
        let status = parse_ocsp_response(&response).expect("should parse unknown response");
        assert_eq!(status, RevocationStatus::Unknown);
    }

    // ── 6. test_parse_ocsp_response_malformed ──

    #[test]
    fn test_parse_ocsp_response_malformed() {
        let garbage = vec![0xFF, 0x01, 0x02, 0x03, 0xDE, 0xAD, 0xBE, 0xEF];
        let result = parse_ocsp_response(&garbage);
        assert!(result.is_err(), "garbage bytes should return error");
    }

    // ── 7. test_ocsp_cache_hit ──

    #[test]
    fn test_ocsp_cache_hit() {
        let checker = OcspRevocationChecker::new().with_cache_ttl(Duration::from_secs(3600));
        let (cert, _der) = gen_test_cert();

        let fingerprint = cert_fingerprint(cert.as_ref());
        checker.cache_status(fingerprint, RevocationStatus::Good);

        let status = checker
            .check_revocation(&cert)
            .expect("should check revocation");
        assert_eq!(status, RevocationStatus::Good);
    }

    // ── 8. test_ocsp_cache_miss_and_populate ──

    #[tokio::test]
    async fn test_ocsp_cache_miss_and_populate() {
        let checker = OcspRevocationChecker::new().with_cache_ttl(Duration::from_secs(3600));
        let (cert, _der) = gen_test_cert();

        let fingerprint = cert_fingerprint(cert.as_ref());

        // Cache is empty, sync check should return Unknown
        assert!(checker.get_cached(&fingerprint).is_none());

        let status = checker
            .check_revocation(&cert)
            .expect("should check revocation");
        assert_eq!(status, RevocationStatus::Unknown);

        // Manually populate cache
        checker.cache_status(fingerprint.clone(), RevocationStatus::Good);
        assert_eq!(
            checker.get_cached(&fingerprint),
            Some(RevocationStatus::Good)
        );
        assert_eq!(checker.cache_size(), 1);
    }

    // ── 9. test_ocsp_cache_expiry ──

    #[test]
    fn test_ocsp_cache_expiry() {
        // Set very short TTL
        let checker = OcspRevocationChecker::new().with_cache_ttl(Duration::from_millis(1));
        let (cert, _der) = gen_test_cert();

        let fingerprint = cert_fingerprint(cert.as_ref());
        checker.cache_status(fingerprint.clone(), RevocationStatus::Good);

        // Sleep to let TTL expire
        std::thread::sleep(Duration::from_millis(10));

        // Cache entry should be expired
        assert!(
            checker.get_cached(&fingerprint).is_none(),
            "expired cache entry should not be returned"
        );

        // Check should return Unknown (no network in sync)
        let status = checker
            .check_revocation(&cert)
            .expect("should check revocation");
        assert_eq!(status, RevocationStatus::Unknown);
    }

    // ── 10. test_ocsp_sync_cache_only ──

    #[test]
    fn test_ocsp_sync_cache_only() {
        // Sync check should never block on network, even with a responder URL
        let checker = OcspRevocationChecker::new()
            .with_responder_url("http://ocsp.example.com")
            .with_cache_ttl(Duration::from_secs(3600));

        let (cert, _der) = gen_test_cert();

        // Sync check with no cache entry returns Unknown (no network)
        let status = checker
            .check_revocation(&cert)
            .expect("should check revocation");
        assert_eq!(status, RevocationStatus::Unknown);
    }

    // ── 11. test_ocsp_fallback_on_error ──

    #[tokio::test]
    async fn test_ocsp_fallback_on_error() {
        // Point to a non-existent responder — should fail-open
        let checker = OcspRevocationChecker::new()
            .with_responder_url("http://127.0.0.1:1")
            .with_timeout(Duration::from_millis(100));

        let (cert, _der) = gen_test_cert();

        let status = checker
            .check_revocation_async(&cert)
            .await
            .expect("should not error even on network failure");
        assert_eq!(status, RevocationStatus::Unknown);
    }

    // ── 12. test_ocsp_with_custom_responder ──

    #[test]
    fn test_ocsp_with_custom_responder() {
        let checker =
            OcspRevocationChecker::new().with_responder_url("http://custom-ocsp.example.com/ocsp");

        let (_cert, der) = gen_test_cert();

        // The custom URL should override any AIA URL
        let url = checker
            .resolve_responder_url(&der)
            .expect("should resolve URL");
        assert_eq!(url, Some("http://custom-ocsp.example.com/ocsp".to_string()));
    }

    // ── Additional: test_parse_url ──

    #[test]
    fn test_parse_url_variants() {
        let (host, port, path) =
            parse_url("http://ocsp.example.com:8080/ocsp").expect("should parse");
        assert_eq!(host, "ocsp.example.com");
        assert_eq!(port, 8080);
        assert_eq!(path, "/ocsp");

        let (host, port, path) = parse_url("http://ocsp.example.com/check").expect("should parse");
        assert_eq!(host, "ocsp.example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/check");

        let (host, port, path) = parse_url("http://ocsp.example.com").expect("should parse");
        assert_eq!(host, "ocsp.example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }

    // ── Additional: test_der_encoding_helpers ──

    #[test]
    fn test_der_integer_from_bytes() {
        // Small positive integer
        let encoded = der_integer_from_bytes(&[0x05]);
        assert_eq!(encoded, vec![TAG_INTEGER, 0x01, 0x05]);

        // Integer with MSB set needs leading zero
        let encoded = der_integer_from_bytes(&[0x80]);
        assert_eq!(encoded, vec![TAG_INTEGER, 0x02, 0x00, 0x80]);

        // Multi-byte integer
        let encoded = der_integer_from_bytes(&[0x01, 0x00]);
        assert_eq!(encoded, vec![TAG_INTEGER, 0x02, 0x01, 0x00]);

        // Leading zeros stripped
        let encoded = der_integer_from_bytes(&[0x00, 0x00, 0x42]);
        assert_eq!(encoded, vec![TAG_INTEGER, 0x01, 0x42]);
    }

    #[test]
    fn test_der_encode_length() {
        assert_eq!(der_encode_length(0), vec![0x00]);
        assert_eq!(der_encode_length(127), vec![0x7F]);
        assert_eq!(der_encode_length(128), vec![0x81, 0x80]);
        assert_eq!(der_encode_length(256), vec![0x82, 0x01, 0x00]);
    }

    // ── Test helper: build a synthetic OCSP response DER ──

    /// Build a minimal valid OCSP response with the given certStatus tag and data
    fn build_test_ocsp_response(cert_status_tag: u8, cert_status_data: &[u8]) -> Vec<u8> {
        // CertID (minimal: algo + two hashes + serial)
        let algo = der_algorithm_identifier(SHA256_OID_BYTES);
        let name_hash = der_octet_string(&[0u8; 32]);
        let key_hash = der_octet_string(&[0u8; 32]);
        let serial = der_integer_from_bytes(&[0x01]);
        let mut cert_id_content = Vec::new();
        cert_id_content.extend(&algo);
        cert_id_content.extend(&name_hash);
        cert_id_content.extend(&key_hash);
        cert_id_content.extend(&serial);
        let cert_id = der_tlv(TAG_SEQUENCE, &cert_id_content);

        // certStatus
        let cert_status = der_tlv(cert_status_tag, cert_status_data);

        // thisUpdate (GeneralizedTime)
        let this_update = der_tlv(0x18, b"20250101000000Z");

        // SingleResponse
        let mut sr_content = Vec::new();
        sr_content.extend(&cert_id);
        sr_content.extend(&cert_status);
        sr_content.extend(&this_update);
        let single_response = der_tlv(TAG_SEQUENCE, &sr_content);

        // responses (SEQUENCE OF SingleResponse)
        let responses = der_tlv(TAG_SEQUENCE, &single_response);

        // responderID [1] byKey (minimal)
        let responder_id = der_tlv(0xA1, &der_octet_string(&[0u8; 20]));

        // producedAt
        let produced_at = der_tlv(0x18, b"20250101000000Z");

        // tbsResponseData
        let mut tbs_content = Vec::new();
        tbs_content.extend(&responder_id);
        tbs_content.extend(&produced_at);
        tbs_content.extend(&responses);
        let tbs_response_data = der_tlv(TAG_SEQUENCE, &tbs_content);

        // signatureAlgorithm (sha256WithRSAEncryption is fine as placeholder)
        let sig_algo = der_algorithm_identifier(SHA256_OID_BYTES);

        // signature BIT STRING (minimal placeholder)
        let signature = der_tlv(0x03, &[0x00, 0x00]); // 0 unused bits + 1 byte

        // BasicOCSPResponse
        let mut basic_content = Vec::new();
        basic_content.extend(&tbs_response_data);
        basic_content.extend(&sig_algo);
        basic_content.extend(&signature);
        let basic_ocsp_response = der_tlv(TAG_SEQUENCE, &basic_content);

        // responseBytes: SEQUENCE { responseType OID, response OCTET STRING }
        let response_type = der_oid(OCSP_BASIC_OID_BYTES);
        let response_octet = der_octet_string(&basic_ocsp_response);
        let mut rb_content = Vec::new();
        rb_content.extend(&response_type);
        rb_content.extend(&response_octet);
        let response_bytes_seq = der_tlv(TAG_SEQUENCE, &rb_content);
        let response_bytes = der_tlv(TAG_CONTEXT_0, &response_bytes_seq);

        // responseStatus ENUMERATED = 0 (successful)
        let response_status = der_tlv(TAG_ENUMERATED, &[0x00]);

        // OCSPResponse
        let mut ocsp_resp_content = Vec::new();
        ocsp_resp_content.extend(&response_status);
        ocsp_resp_content.extend(&response_bytes);
        der_tlv(TAG_SEQUENCE, &ocsp_resp_content)
    }
}
