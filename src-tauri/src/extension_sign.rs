//! Signatures of `.mcpb` bundles, in the format `mcpb sign` writes: a
//! detached PKCS#7 SignedData over the zip, appended as `MCPB_SIG_V1`, its
//! length (u32, little-endian), the DER, `MCPB_SIG_END`; the zip's EOCD
//! comment length is grown to cover the block, after signing. Checked here:
//! the signed message digest against the bundle's bytes, the signer's RSA or
//! ECDSA signature, and the certificate chain to the system's roots for code
//! signing. Only what was checked is claimed (see `Check::status`).

use crate::error::AppResult;
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::OnceLock;

const HEADER: &[u8] = b"MCPB_SIG_V1";
const FOOTER: &[u8] = b"MCPB_SIG_END";
/// The signature block is read from the file's end: a PKCS#7 with a few certificates is a few kB.
const MAX_TAIL: u64 = 1024 * 1024;

const SEQ: u8 = 0x30;
const SET: u8 = 0x31;
const OID: u8 = 0x06;
const INT: u8 = 0x02;
const OCTETS: u8 = 0x04;
const CTX0: u8 = 0xA0;
const CTX1: u8 = 0xA1;

const SIGNED_DATA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];
const SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const MESSAGE_DIGEST: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x04];
const RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
const RSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B];
const ECDSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
const CN: &[u8] = &[0x55, 0x04, 0x03];
const ORG: &[u8] = &[0x55, 0x04, 0x0A];
/// id-kp-codeSigning, 1.3.6.1.5.5.7.3.3
const CODE_SIGNING: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x03];

/// One DER element: its tag, its contents, and the whole of it.
struct Tlv<'a> {
    tag: u8,
    body: &'a [u8],
    all: &'a [u8],
}

fn next<'a>(input: &mut &'a [u8]) -> Option<Tlv<'a>> {
    let s = *input;
    let (&tag, rest) = s.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (first as usize, rest)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        (rest[..n].iter().fold(0usize, |a, &b| (a << 8) | b as usize), &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    let head = s.len() - rest.len();
    *input = &rest[len..];
    Some(Tlv { tag, body: &rest[..len], all: &s[..head + len] })
}

fn expect<'a>(input: &mut &'a [u8], tag: u8) -> Option<Tlv<'a>> {
    next(input).filter(|t| t.tag == tag)
}

/// The OID of an AlgorithmIdentifier's contents.
fn algorithm(mut alg: &[u8]) -> Option<&[u8]> {
    expect(&mut alg, OID).map(|t| t.body)
}

/// What a SignedData says about its first signer.
struct Signed<'a> {
    certs: Vec<&'a [u8]>,
    issuer: &'a [u8],
    serial: &'a [u8],
    digest_alg: &'a [u8],
    attrs: Option<&'a [u8]>,
    sig_alg: &'a [u8],
    sig: &'a [u8],
}

fn parse(der: &[u8]) -> Option<Signed<'_>> {
    let mut d = der;
    let mut ci = expect(&mut d, SEQ)?.body;
    if expect(&mut ci, OID)?.body != SIGNED_DATA {
        return None;
    }
    let mut wrapped = expect(&mut ci, CTX0)?.body;
    let mut sd = expect(&mut wrapped, SEQ)?.body;
    expect(&mut sd, INT)?;
    expect(&mut sd, SET)?;
    expect(&mut sd, SEQ)?;
    let mut t = next(&mut sd)?;
    let mut certs = vec![];
    if t.tag == CTX0 {
        let mut c = t.body;
        while !c.is_empty() {
            certs.push(expect(&mut c, SEQ)?.all);
        }
        t = next(&mut sd)?;
    }
    if t.tag == CTX1 {
        t = next(&mut sd)?;
    }
    if t.tag != SET {
        return None;
    }
    let mut infos = t.body;
    let mut si = expect(&mut infos, SEQ)?.body;
    expect(&mut si, INT)?;
    // issuerAndSerialNumber; a signer named by key identifier isn't supported
    let mut sid = expect(&mut si, SEQ)?.body;
    let issuer = expect(&mut sid, SEQ)?.body;
    let serial = expect(&mut sid, INT)?.body;
    let digest_alg = algorithm(expect(&mut si, SEQ)?.body)?;
    let mut t = next(&mut si)?;
    let attrs = if t.tag == CTX0 {
        let a = t.all;
        t = next(&mut si)?;
        Some(a)
    } else {
        None
    };
    if t.tag != SEQ {
        return None;
    }
    let sig_alg = algorithm(t.body)?;
    let sig = expect(&mut si, OCTETS)?.body;
    Some(Signed { certs, issuer, serial, digest_alg, attrs, sig_alg, sig })
}

/// The messageDigest attribute among signed attributes `attrs` ([0] and all).
fn message_digest(attrs: &[u8]) -> Option<&[u8]> {
    let mut a = expect(&mut &attrs[..], CTX0)?.body;
    while !a.is_empty() {
        let mut attr = expect(&mut a, SEQ)?.body;
        if expect(&mut attr, OID)?.body == MESSAGE_DIGEST {
            let mut values = expect(&mut attr, SET)?.body;
            return expect(&mut values, OCTETS).map(|t| t.body);
        }
    }
    None
}

/// "Common Name (Organization)" of a certificate Name's contents.
fn display_name(mut name: &[u8]) -> String {
    let (mut cn, mut org) = (None, None);
    while let Some(rdn) = expect(&mut name, SET) {
        let mut set = rdn.body;
        while let Some(atv) = expect(&mut set, SEQ) {
            let mut atv = atv.body;
            let (Some(oid), Some(v)) = (expect(&mut atv, OID), next(&mut atv)) else { continue };
            // BMPString is UTF-16; every other string type here is ASCII or UTF-8
            let text = if v.tag == 0x1E {
                String::from_utf16_lossy(&v.body.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect::<Vec<_>>())
            } else {
                String::from_utf8_lossy(v.body).into_owned()
            };
            match oid.body {
                CN => cn = Some(text),
                ORG => org = Some(text),
                _ => {}
            }
        }
    }
    match (cn, org) {
        (Some(c), Some(o)) if c != o => format!("{c} ({o})"),
        (Some(c), _) => c,
        (None, Some(o)) => o,
        (None, None) => "?".into(),
    }
}

/// The result of checking a bundle.
#[derive(Debug, PartialEq)]
pub struct Check {
    /// "unsigned"; "invalid" (the signature doesn't match the file: changed
    /// after signing); "unverifiable" (signed, in a form not checked here);
    /// or, the signature checked: "self_signed", "untrusted" (no chain to a
    /// trusted root for code signing) or "trusted".
    pub status: &'static str,
    pub publisher: Option<String>,
    pub issuer: Option<String>,
    /// SHA-256 of the signing certificate.
    pub fingerprint: Option<String>,
}

impl Check {
    fn bare(status: &'static str) -> Self {
        Check { status, publisher: None, issuer: None, fingerprint: None }
    }

    pub fn to_json(&self) -> Value {
        json!({"status": self.status, "publisher": self.publisher, "issuer": self.issuer, "fingerprint": self.fingerprint})
    }
}

/// Where the signature block of a file of `len` bytes ending in `tail` is:
/// the offset the block starts at and the DER it holds.
fn block(tail: &[u8], len: u64) -> Option<(u64, &[u8])> {
    let body = tail.strip_suffix(FOOTER)?;
    let at = body.windows(HEADER.len()).rposition(|w| w == HEADER)?;
    let rest = &body[at + HEADER.len()..];
    let n = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
    let der = rest.get(4..)?;
    (der.len() == n).then_some((len - tail.len() as u64 + at as u64, der))
}

type Sha = [u8; 32];

/// SHA-256 of the file's first `content` bytes as they are and, when the
/// zip's EOCD comment length at `patch` was grown over the block, as they
/// were signed; and of the whole file.
fn digests(f: &mut std::fs::File, content: u64, patch: Option<(u64, [u8; 2])>) -> std::io::Result<(Sha, Option<Sha>, Sha)> {
    f.seek(SeekFrom::Start(0))?;
    let (mut plain, mut signed, mut whole) = (Sha256::new(), patch.map(|_| Sha256::new()), Sha256::new());
    let mut buf = vec![0u8; 64 * 1024];
    let mut pos = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &mut buf[..n];
        whole.update(&*chunk);
        let inside = (content.saturating_sub(pos) as usize).min(n);
        plain.update(&chunk[..inside]);
        if let (Some(h), Some((at, bytes))) = (signed.as_mut(), patch) {
            for (i, b) in bytes.iter().enumerate() {
                let p = at + i as u64;
                if (pos..pos + inside as u64).contains(&p) {
                    chunk[(p - pos) as usize] = *b;
                }
            }
            h.update(&chunk[..inside]);
        }
        pos += n as u64;
    }
    Ok((plain.finalize().into(), signed.map(|h| h.finalize().into()), whole.finalize().into()))
}

/// The EOCD comment length as it was before `mcpb sign` grew it by the
/// `block` bytes after `content`: its offset and old value, when `tail`
/// (the file's last bytes, starting at `tail_at`) holds such an EOCD.
fn eocd_patch(tail: &[u8], tail_at: u64, content: u64, len: u64) -> Option<(u64, [u8; 2])> {
    let before = &tail[..(content - tail_at) as usize];
    let at = before.windows(4).rposition(|w| w == b"PK\x05\x06")?;
    let comment = u16::from_le_bytes(before.get(at + 20..at + 22)?.try_into().ok()?) as u64;
    let eocd = tail_at + at as u64;
    let grown_by = len - content;
    (eocd + 22 + comment == len && comment >= grown_by).then(|| (eocd + 20, ((comment - grown_by) as u16).to_le_bytes()))
}

fn system_roots() -> &'static [TrustAnchor<'static>] {
    static ROOTS: OnceLock<Vec<TrustAnchor<'static>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        rustls_native_certs::load_native_certs()
            .certs
            .iter()
            .filter_map(|c| webpki::anchor_from_trusted_cert(c).ok().map(|a| a.to_owned()))
            .collect()
    })
}

/// The signature of the bundle at `path`, checked against the system's
/// roots, and the SHA-256 of the whole file.
pub fn check_file(path: &Path) -> AppResult<(Check, String)> {
    check_with(path, system_roots())
}

fn check_with(path: &Path, roots: &[TrustAnchor<'_>]) -> AppResult<(Check, String)> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let tail_at = len.saturating_sub(MAX_TAIL);
    let mut tail = vec![];
    f.seek(SeekFrom::Start(tail_at))?;
    f.by_ref().take(MAX_TAIL).read_to_end(&mut tail)?;
    let Some((content, der)) = block(&tail, len) else {
        let (_, _, whole) = digests(&mut f, len, None)?;
        return Ok((Check::bare("unsigned"), hex::encode(whole)));
    };
    let patch = eocd_patch(&tail, tail_at, content, len);
    let (plain, signed, whole) = digests(&mut f, content, patch)?;
    Ok((verify(der, &[Some(plain), signed].into_iter().flatten().collect::<Vec<_>>(), roots), hex::encode(whole)))
}

/// Check SignedData `der` over content whose SHA-256 is one of `digests`.
fn verify(der: &[u8], digests: &[[u8; 32]], roots: &[TrustAnchor<'_>]) -> Check {
    let Some(s) = parse(der) else { return Check::bare("unverifiable") };
    let certs: Vec<CertificateDer<'_>> = s.certs.iter().map(|c| CertificateDer::from(*c)).collect();
    let signer = certs.iter().find_map(|c| {
        let ee = webpki::EndEntityCert::try_from(c).ok()?;
        (ee.issuer() == s.issuer && ee.serial() == s.serial).then_some((c, ee))
    });
    let Some((cert, ee)) = signer else { return Check::bare("unverifiable") };
    let mut out = Check {
        status: "unverifiable",
        publisher: Some(display_name(ee.subject())),
        issuer: Some(display_name(ee.issuer())),
        fingerprint: Some(hex::encode(Sha256::digest(cert.as_ref()))),
    };
    let alg: &dyn rustls_pki_types::SignatureVerificationAlgorithm = match s.sig_alg {
        RSA | RSA_SHA256 => webpki::ring::RSA_PKCS1_2048_8192_SHA256,
        ECDSA_SHA256 => webpki::ring::ECDSA_P256_SHA256,
        _ => return out,
    };
    if s.digest_alg != SHA256 {
        return out;
    }
    // with signed attributes the signature is over them (as a SET) and they hold the content's digest
    let signed_attrs;
    let msg: &[u8] = match s.attrs {
        Some(a) => {
            if !message_digest(a).is_some_and(|d| digests.iter().any(|x| x[..] == *d)) {
                out.status = "invalid";
                return out;
            }
            signed_attrs = [&[SET][..], &a[1..]].concat();
            &signed_attrs
        }
        // shortcut: without signed attributes the signature is over the whole content, which isn't kept in memory here
        None => return out,
    };
    if ee.verify_signature(alg, msg, s.sig).is_err() {
        out.status = "invalid";
        return out;
    }
    let intermediates: Vec<CertificateDer<'_>> = certs.iter().filter(|c| *c != cert).cloned().collect();
    let algs = [webpki::ring::RSA_PKCS1_2048_8192_SHA256, webpki::ring::RSA_PKCS1_2048_8192_SHA384, webpki::ring::RSA_PKCS1_2048_8192_SHA512, webpki::ring::ECDSA_P256_SHA256, webpki::ring::ECDSA_P384_SHA384];
    out.status = if ee.issuer() == ee.subject() {
        "self_signed"
    } else if ee.verify_for_usage(&algs, roots, &intermediates, UnixTime::now(), webpki::KeyUsage::required(CODE_SIGNING), None, None).is_ok() {
        "trusted"
    } else {
        "untrusted"
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNED_CA: &[u8] = include_bytes!("testdata/signed_ca.mcpb");
    const SIGNED_SELF: &[u8] = include_bytes!("testdata/signed_self.mcpb");
    const TEST_ROOT: &[u8] = include_bytes!("testdata/test_root.der");

    fn check_bytes(tag: &str, bytes: &[u8], roots: &[TrustAnchor<'_>]) -> (Check, String) {
        let p = std::env::temp_dir().join(format!("sig-{tag}-{}.mcpb", crate::util::new_id()));
        std::fs::write(&p, bytes).unwrap();
        let r = check_with(&p, roots).unwrap();
        let _ = std::fs::remove_file(&p);
        r
    }

    fn root() -> TrustAnchor<'static> {
        webpki::anchor_from_trusted_cert(&CertificateDer::from(TEST_ROOT)).unwrap().to_owned()
    }

    #[test]
    fn a_bundle_signed_through_a_trusted_root_is_trusted_and_names_its_publisher() {
        let (c, sha) = check_bytes("ca", SIGNED_CA, &[root()]);
        assert_eq!(c.status, "trusted", "{c:?}");
        assert_eq!(c.publisher.as_deref(), Some("Example Publisher (Example Inc)"));
        assert_eq!(c.issuer.as_deref(), Some("ClaudeRust Test Root"));
        assert_eq!(c.fingerprint.as_ref().map(String::len), Some(64));
        assert_eq!(sha, hex::encode(Sha256::digest(SIGNED_CA)), "the hash is of the whole file");
        // the same signature, but no root the system trusts
        assert_eq!(check_bytes("ca2", SIGNED_CA, &[]).0.status, "untrusted");
    }

    #[test]
    fn a_self_signed_bundle_says_so() {
        let c = check_bytes("self", SIGNED_SELF, &[root()]).0;
        assert_eq!((c.status, c.publisher.as_deref()), ("self_signed", Some("Self Publisher")));
    }

    #[test]
    fn a_bundle_changed_after_signing_is_invalid() {
        let mut b = SIGNED_CA.to_vec();
        let at = b.windows(8).position(|w| w == b"'signed'").unwrap() + 1;
        b[at] ^= 1;
        assert_eq!(check_bytes("bad", &b, &[root()]).0.status, "invalid");
    }

    #[test]
    fn unsigned_and_damaged_blocks() {
        let content_len = SIGNED_CA.windows(HEADER.len()).rposition(|w| w == HEADER).unwrap();
        let (c, sha) = check_bytes("plain", &SIGNED_CA[..content_len], &[root()]);
        assert_eq!(c, Check::bare("unsigned"));
        assert_eq!(sha, hex::encode(Sha256::digest(&SIGNED_CA[..content_len])));
        // a block whose length doesn't add up, or whose DER is garbage
        let mut wrong_len = SIGNED_CA.to_vec();
        wrong_len[content_len + HEADER.len()] ^= 1;
        assert_eq!(check_bytes("len", &wrong_len, &[]).0.status, "unsigned");
        let garbage = [&SIGNED_CA[..content_len], HEADER, &3u32.to_le_bytes(), b"xyz", FOOTER].concat();
        assert_eq!(check_bytes("junk", &garbage, &[]).0.status, "unverifiable");
        // the footer must end the file
        assert_eq!(check_bytes("trail", &[SIGNED_CA, b"\n"].concat(), &[]).0.status, "unsigned");
    }

    #[test]
    fn a_signed_bundle_still_opens_as_a_zip() {
        let p = std::env::temp_dir().join(format!("sig-zip-{}.mcpb", crate::util::new_id()));
        std::fs::write(&p, SIGNED_CA).unwrap();
        let info = crate::extensions::inspect(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(info["name"], "signed-demo");
    }

    #[test]
    fn der_lengths_are_bounded() {
        assert!(next(&mut &[0x30, 0x85, 1, 2, 3, 4, 5][..]).is_none(), "5 length bytes");
        assert!(next(&mut &[0x30, 0x82, 0xff, 0xff, 1][..]).is_none(), "longer than the input");
        let mut s: &[u8] = &[0x04, 0x02, 9, 8, 0x05, 0x00];
        let t = next(&mut s).unwrap();
        assert_eq!((t.tag, t.body, t.all.len(), s), (0x04, &[9u8, 8][..], 4, &[0x05u8, 0x00][..]));
    }
}
