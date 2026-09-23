use std::{io, path::Path};

fn pem_blocks(bytes: &[u8], kind: &str) -> io::Result<Vec<u8>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("TLS identity requires PEM {kind} material"),
        )
    };
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let begin = format!("-----BEGIN {kind}-----");
    let end = format!("-----END {kind}-----");
    let mut rest = text;
    let mut result = Vec::new();
    while let Some(start) = rest.find(&begin) {
        rest = &rest[start..];
        let stop = rest.find(&end).ok_or_else(invalid)? + end.len();
        result.extend_from_slice(&rest.as_bytes()[..stop]);
        result.push(b'\n');
        rest = &rest[stop..];
    }
    if result.is_empty() {
        return Err(invalid());
    }
    Ok(result)
}

pub(super) fn identity(
    certfile: &Path,
    keyfile: &Path,
) -> Result<native_tls::Identity, Box<dyn std::error::Error + Send + Sync>> {
    let cert = pem_blocks(&std::fs::read(certfile)?, "CERTIFICATE")?;
    // Extract the key so certificate-first combined PEM files work too.
    // native-tls requires an unencrypted PKCS#8 key on every supported backend.
    let key = pem_blocks(&std::fs::read(keyfile)?, "PRIVATE KEY").map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "TLS client key must be unencrypted PKCS#8 PEM; convert legacy keys with openssl pkcs8 -topk8 -nocrypt (see upgrade-from-python-recipe.md)")
    })?;
    Ok(native_tls::Identity::from_pkcs8(&cert, &key)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_chain_and_key_regardless_of_combined_file_order() {
        let block = |kind: &str, data: &str| {
            format!("-----BEGIN {kind}-----\n{data}\n-----END {kind}-----\n")
        };
        let certs = block("CERTIFICATE", "leaf") + &block("CERTIFICATE", "issuer");
        let key = block("PRIVATE KEY", "synthetic");
        for combined in [certs.clone() + &key, key.clone() + &certs] {
            assert_eq!(
                pem_blocks(combined.as_bytes(), "CERTIFICATE").unwrap(),
                certs.as_bytes()
            );
            assert_eq!(
                pem_blocks(combined.as_bytes(), "PRIVATE KEY").unwrap(),
                key.as_bytes()
            );
        }
        let invalid = block("RSA PRIVATE KEY", "synthetic");
        let error = pem_blocks(invalid.as_bytes(), "PRIVATE KEY").unwrap_err();
        assert!(!error.to_string().contains("synthetic"));
    }
}
