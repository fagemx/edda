use super::types::*;
use super::validate::{validate_capsule, validate_event_id, validate_portable_repo_id};
use crate::canon::canonical_json_bytes;
use crate::hash::sha256_hex;

pub fn canonical_capsule_bytes(capsule: &ContextCapsuleV1) -> anyhow::Result<Vec<u8>> {
    validate_capsule(capsule)?;
    let bytes = canonical_json_bytes(&serde_json::to_value(capsule)?)?;
    if bytes.len() > MAX_CONTINUITY_INPUT_BYTES {
        anyhow::bail!("canonical capsule exceeds its byte bound");
    }
    Ok(bytes)
}

pub fn capsule_digest(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

pub fn decode_capsule_bytes(
    bytes_hex: &str,
    expected_digest: &str,
) -> anyhow::Result<(Vec<u8>, ContextCapsuleV1)> {
    if bytes_hex.len() > MAX_CONTINUITY_INPUT_BYTES * 2 {
        anyhow::bail!("capsule_bytes_hex exceeds the capsule bound");
    }
    let bytes =
        hex::decode(bytes_hex).map_err(|_| anyhow::anyhow!("capsule_bytes_hex is invalid"))?;
    if capsule_digest(&bytes) != expected_digest {
        anyhow::bail!("capsule_sha256 does not match capsule bytes");
    }
    let capsule: ContextCapsuleV1 = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("capsule bytes are not a supported ContextCapsuleV1"))?;
    validate_capsule(&capsule)?;
    if canonical_capsule_bytes(&capsule)? != bytes {
        anyhow::bail!("capsule bytes must use canonical JSON encoding");
    }
    Ok((bytes, capsule))
}

pub fn make_bundle(record: &CapsuleRecordV1) -> anyhow::Result<PortableCapsuleBundleV1> {
    validate_record(record)?;
    let mut bundle = PortableCapsuleBundleV1 {
        bundle_version: CONTINUITY_BUNDLE_VERSION,
        portable_repo_id: record
            .origin
            .portable_repo_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("local-only capsules cannot be exported"))?,
        origin_capsule_id: record.origin.capsule_id.clone(),
        origin_event_id: record.origin.event_id.clone(),
        capsule_sha256: record.capsule_sha256.clone(),
        capsule_bytes_hex: record.capsule_bytes_hex.clone(),
        bundle_sha256: String::new(),
        data_authority: DataAuthority::DataOnly,
    };
    bundle.bundle_sha256 = compute_bundle_digest(&bundle)?;
    Ok(bundle)
}

pub fn validate_bundle(bundle: &PortableCapsuleBundleV1) -> anyhow::Result<ContextCapsuleV1> {
    if bundle.bundle_version != CONTINUITY_BUNDLE_VERSION {
        anyhow::bail!("unsupported bundle_version (expected 1)");
    }
    validate_portable_repo_id(&bundle.portable_repo_id)?;
    validate_event_id(&bundle.origin_event_id)?;
    if compute_bundle_digest(bundle)? != bundle.bundle_sha256 {
        anyhow::bail!("bundle_sha256 does not match bundle content");
    }
    let (_, capsule) = decode_capsule_bytes(&bundle.capsule_bytes_hex, &bundle.capsule_sha256)?;
    if capsule.capsule_id != bundle.origin_capsule_id {
        anyhow::bail!("bundle origin_capsule_id conflicts with capsule bytes");
    }
    if capsule.repository.portable_repo_id.as_deref() != Some(&bundle.portable_repo_id) {
        anyhow::bail!("bundle portable_repo_id conflicts with capsule bytes");
    }
    Ok(capsule)
}

pub fn validate_record(record: &CapsuleRecordV1) -> anyhow::Result<Vec<u8>> {
    if record.record_version != CONTINUITY_RECORD_VERSION {
        anyhow::bail!("unsupported continuity record_version");
    }
    validate_event_id(&record.origin.event_id)?;
    if let Some(portable_repo_id) = &record.origin.portable_repo_id {
        validate_portable_repo_id(portable_repo_id)?;
    }
    let (bytes, capsule) = decode_capsule_bytes(&record.capsule_bytes_hex, &record.capsule_sha256)?;
    if capsule != record.capsule
        || capsule.capsule_id != record.origin.capsule_id
        || capsule.repository.portable_repo_id != record.origin.portable_repo_id
    {
        anyhow::bail!("continuity record identity or capsule projection conflicts with bytes");
    }
    Ok(bytes)
}

fn compute_bundle_digest(bundle: &PortableCapsuleBundleV1) -> anyhow::Result<String> {
    let mut value = serde_json::to_value(bundle)?;
    value
        .as_object_mut()
        .expect("bundle serializes as an object")
        .remove("bundle_sha256");
    Ok(sha256_hex(&canonical_json_bytes(&value)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::continuity::{build_capsule, CapsuleGitV1, CapsuleRepositoryV1, ContextSourceV1};

    fn input() -> ContextCapsuleInputV1 {
        serde_json::from_value(serde_json::json!({
            "capsule_version": 1,
            "state": {"next_action": "run the focused test"}
        }))
        .unwrap()
    }

    #[test]
    fn bundle_detects_identity_tampering() {
        let capsule = build_capsule(
            input(),
            ContextSourceV1::default(),
            CapsuleRepositoryV1 {
                portable_repo_id: Some(format!("repo_{}", "a".repeat(64))),
                display_hint: Some("example.com/org/repo".into()),
                local_only_reason: None,
            },
            CapsuleGitV1::default(),
        )
        .unwrap();
        let bytes = canonical_capsule_bytes(&capsule).unwrap();
        let record = CapsuleRecordV1 {
            record_version: 1,
            data_authority: DataAuthority::DataOnly,
            origin: CapsuleOriginV1 {
                capsule_id: capsule.capsule_id.clone(),
                event_id: format!("evt_{}", "1".repeat(26)),
                portable_repo_id: capsule.repository.portable_repo_id.clone(),
            },
            capsule_sha256: capsule_digest(&bytes),
            capsule_bytes_hex: hex::encode(bytes),
            capsule,
            imported: false,
        };
        let mut bundle = make_bundle(&record).unwrap();
        bundle.origin_event_id = format!("evt_{}", "2".repeat(26));
        assert!(validate_bundle(&bundle).is_err());
    }
}
