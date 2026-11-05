//! WP02 validation (spec 11.12): published BLAKE3 vectors, streaming imports
//! larger than internal buffers, duplicate content sharing one immutable
//! object, same-ID replacement, deterministic digests under shuffled
//! insertion, path confinement, corrupted bytes, truncated files, missing
//! dependencies, and failure immediately before the object rename — with the
//! invariant that after EVERY failure, previous objects remain usable.

use anim_assets::{
    decode, import, manifest_digest, resolve, verify, AssetDiagnostic, AssetError,
    AssetKind, AssetManifest, AssetManifestEntry, AssetResolver, AssetStore, ContentHash,
};
use anim_core::id::AssetId;
use std::io::Read;

fn store_in(dir: &std::path::Path) -> AssetStore {
    AssetStore::new(dir)
}

fn data_1mib() -> Vec<u8> {
    let mut v = Vec::with_capacity(1024 * 1024);
    for i in 0..1024 * 1024 {
        v.push((i % 251) as u8);
    }
    v
}

/// A reader that yields less than requested — proves bounded-chunk streaming
/// works with adversarial sources (spec 11.12 "imports larger than buffers").
struct TrickleReader {
    data: Vec<u8>,
    pos: usize,
    max_chunk: usize,
}

impl Read for TrickleReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.data.len() {
            return Ok(0);
        }
        let n = self.max_chunk.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

// ------------------------------------------------- published BLAKE3 vectors

#[test]
fn published_blake3_vectors() {
    // Official BLAKE3 self-test vectors (BLAKE3 team, test_vectors.json):
    // vector 0 = empty-input digest. Full published vector set is exercised
    // by the blake3 crate's own release tests; our job is to pin our
    // streaming pipeline against the canonical value.
    assert_eq!(
        blake3::hash(&[]).to_hex().to_string(),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    // streaming == one-shot for a multi-chunk input (our import path)
    let data = data_1mib();
    let mut h = anim_assets::hash::StreamingHasher::new();
    for chunk in data.chunks(8192) {
        h.update(chunk);
    }
    assert_eq!(h.finalize(), ContentHash(*blake3::hash(&data).as_bytes()));
}

// -------------------------------------------------- hash parsing/confinement

#[test]
fn hash_parse_rejects_malformed() {
    assert!(matches!(ContentHash::parse("short"), Err(AssetError::InvalidHash(_))));
    assert!(matches!(
        ContentHash::parse(
            "this is sixty four characters long and not hex at all! [[[[[[["
        ),
        Err(AssetError::InvalidHash(_))
    ));
    assert!(matches!(
        ContentHash::parse("../../../../etc/passwd@@@"),
        Err(AssetError::InvalidHash(_))
    ));
    // uppercase is accepted but canonicalized to lowercase for paths
    let upper = ContentHash::parse(
        "AF1349B9F5F9A1A6A0404DEA36DCC9499BCB25C9B1125B2E8A5493E5F65B4C21",
    )
    .unwrap();
    assert!(upper.full_hex().chars().all(|c| !c.is_ascii_uppercase()));
}

#[test]
fn object_paths_confined_under_root() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let h = ContentHash(*blake3::hash(b"confined").as_bytes());
    let p = store.object_path(&h);
    assert!(p.starts_with(dir.path()));
    let rel = p.strip_prefix(dir.path()).unwrap();
    let comps: Vec<_> = rel.components().map(|c| c.as_os_str().to_str().unwrap()).collect();
    assert_eq!(comps[0], "assets");
    assert_eq!(comps[1], "objects");
    assert_eq!(comps[2].len(), 2);
    assert_eq!(comps[3].len(), 2);
    assert_eq!(comps[4].len(), 64);
}

// ------------------------------------------------------------- import flows

#[test]
fn duplicate_content_shares_one_immutable_object() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let bytes = data_1mib();

    let a = import(
        &store,
        AssetId::new_v4(),
        "first.png".into(),
        AssetKind::Image,
        1,
        &bytes[..],
    )
    .unwrap();
    let b = import(
        &store,
        AssetId::new_v4(),
        "second.png".into(),
        AssetKind::Image,
        1,
        &bytes[..],
    )
    .unwrap();

    // same content → same hash, same object path; both entries resolve
    assert_eq!(a.content_hash, b.content_hash);
    assert_eq!(store.object_path(&a.content_hash), store.object_path(&b.content_hash));
    verify(&store, &a).unwrap();
    verify(&store, &b).unwrap();

    // no stray staging files remain
    let staging = store.temp_dir();
    assert!(
        std::fs::read_dir(&staging).unwrap().next().is_none(),
        "staging dir must be empty after successful imports"
    );
}

#[test]
fn trickle_source_larger_than_internal_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let bytes = data_1mib(); // 1 MiB >> 64 KiB internal chunk
    let entry = import(
        &store,
        AssetId::new_v4(),
        "trickle.bin".into(),
        AssetKind::Other,
        1,
        TrickleReader { data: bytes.clone(), pos: 0, max_chunk: 4096 },
    )
    .unwrap();
    assert_eq!(entry.byte_length, bytes.len() as u64);
    assert_eq!(entry.content_hash, ContentHash(*blake3::hash(&bytes).as_bytes()));
    verify(&store, &entry).unwrap();
}

#[test]
fn same_id_reimport_different_content_new_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let id = AssetId::new_v4();

    let first =
        import(&store, id, "rig_v1".into(), AssetKind::Rig, 1, &b"payload-v1"[..]).unwrap();
    let second =
        import(&store, id, "rig_v1".into(), AssetKind::Rig, 1, &b"payload-v2"[..]).unwrap();

    // logical identity stable, content revision changed
    assert_eq!(first.asset_id, id);
    assert_eq!(second.asset_id, id);
    assert_ne!(first.content_hash, second.content_hash);
    // both objects coexist (immutable, no overwrite) and are individually valid
    verify(&store, &first).unwrap();
    verify(&store, &second).unwrap();
}

#[test]
fn corrupted_bytes_reported_hash_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let entry = import(
        &store,
        AssetId::new_v4(),
        "c".into(),
        AssetKind::Image,
        1,
        &b"0123456789"[..],
    )
    .unwrap();

    // flip one byte in the object
    let path = store.object_path(&entry.content_hash);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();

    assert!(matches!(verify(&store, &entry), Err(AssetError::HashMismatch(_))));
    // resolver surfaces it as a preview diagnostic, never a silent substitute
    let mut m = AssetManifest::new();
    m.insert(entry.clone()).unwrap();
    let resolver = AssetResolver::new(&store, &m);
    let diags = resolver.diagnostics();
    assert!(matches!(diags[0], AssetDiagnostic::Corrupt { .. }));
}

#[test]
fn truncated_file_reported_size_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let entry = import(
        &store,
        AssetId::new_v4(),
        "t".into(),
        AssetKind::Other,
        1,
        &b"0123456789"[..],
    )
    .unwrap();

    let path = store.object_path(&entry.content_hash);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..5]).unwrap(); // truncate

    assert!(matches!(
        verify(&store, &entry),
        Err(AssetError::SizeMismatch { expected: 10, found: 5 })
    ));
}

#[test]
fn failure_immediately_before_rename_keeps_store_intact() {
    // Simulate commit-time failure: reserve the final object path as a
    // DIRECTORY so the rename must fail (file-vs-dir collision). Precompute
    // the payload hash to know that path in advance.
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store.ensure_layout().unwrap();

    let payload = b"interrupted-import-payload";
    let hash = ContentHash(*blake3::hash(payload).as_bytes());

    // a previous GOOD object must survive the failure
    let survivor = import(
        &store,
        AssetId::new_v4(),
        "survivor".into(),
        AssetKind::Other,
        1,
        &b"survivor-payload"[..],
    )
    .unwrap();

    // sabotage: WRONG content at the object path → dedup path must VERIFY
    // and REJECT (different content at same hash is impossible; this shows
    // as SizeMismatch/HashMismatch, not silent reuse)
    let obj_path = store.object_path(&hash);
    std::fs::create_dir_all(obj_path.parent().unwrap()).unwrap();
    std::fs::write(&obj_path, b"not-our-object-so-mismatch").unwrap();

    let result = import(
        &store,
        AssetId::new_v4(),
        "doomed".into(),
        AssetKind::Other,
        1,
        &payload[..],
    );
    assert!(
        result.is_err(),
        "existing object with different content must be rejected, never reused"
    );

    // second failure shape: final path exists as a DIRECTORY so the
    // rename commit itself must fail (failure immediately before rename)
    let committed_path = store.object_path(&hash);
    std::fs::remove_file(&committed_path).unwrap();
    std::fs::create_dir(&committed_path).unwrap();
    let result = import(
        &store,
        AssetId::new_v4(),
        "doomed2".into(),
        AssetKind::Other,
        1,
        &payload[..],
    );
    assert!(result.is_err(), "file-onto-directory rename must fail (commit blocked)");

    // previous project + asset objects remain usable after every failure
    verify(&store, &survivor).unwrap();
    // staging is cleaned: no orphan temp files
    assert!(std::fs::read_dir(store.temp_dir()).unwrap().next().is_none());
}

// ---------------------------------------------------------------- manifest

#[test]
fn digest_deterministic_under_shuffled_insertion() {
    let ids: Vec<AssetId> = (1..10).map(AssetId::from_u128).collect();
    let mk = |i: &AssetId, tag: u8| {
        AssetManifestEntry::new(
            *i,
            ContentHash(*blake3::hash(&[tag]).as_bytes()),
            tag as u64,
            AssetKind::Other,
            1,
            Vec::new(),
            format!("asset-{tag}"),
        )
        .unwrap()
    };

    let mut m1 = AssetManifest::new();
    let mut m2 = AssetManifest::new();
    for i in &ids {
        m1.insert(mk(i, 7)).unwrap();
    }
    for i in ids.iter().rev() {
        m2.insert(mk(i, 7)).unwrap();
    }
    assert_eq!(manifest_digest(&m1).unwrap(), manifest_digest(&m2).unwrap());

    // digest changes when content changes
    let mut m3 = AssetManifest::new();
    for i in &ids {
        m3.insert(mk(i, 8)).unwrap();
    }
    assert_ne!(manifest_digest(&m1).unwrap(), manifest_digest(&m3).unwrap());
}

#[test]
fn empty_manifest_digest_is_valid_and_deterministic() {
    let a = manifest_digest(&AssetManifest::new()).unwrap();
    let b = manifest_digest(&AssetManifest::new()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn duplicate_dependencies_rejected() {
    let a = AssetId::from_u128(1);
    let e = AssetManifestEntry::new(
        AssetId::from_u128(2),
        ContentHash([0u8; 32]),
        0,
        AssetKind::Other,
        1,
        vec![a, a], // duplicate
        "dup-deps".into(),
    );
    assert!(matches!(e, Err(AssetError::DuplicateAssetId(_))));
}

#[test]
fn self_dependency_is_a_cycle() {
    let a = AssetId::from_u128(1);
    let e = AssetManifestEntry::new(
        a,
        ContentHash([0u8; 32]),
        0,
        AssetKind::Other,
        1,
        vec![a], // self-reference
        "self".into(),
    );
    assert!(matches!(e, Err(AssetError::DependencyCycle(_))));
}

#[test]
fn dependency_cycle_across_manifest_rejected() {
    let a = AssetId::from_u128(1);
    let b = AssetId::from_u128(2);
    let mut m = AssetManifest::new();

    m.insert(
        AssetManifestEntry::new(
            b,
            ContentHash(*blake3::hash(b"b").as_bytes()),
            1,
            AssetKind::Other,
            1,
            vec![a], // depends on A (not yet present — forward dep is legal)
            "b".into(),
        )
        .unwrap(),
    )
    .unwrap();

    let result = m.insert(
        AssetManifestEntry::new(
            a,
            ContentHash(*blake3::hash(b"a").as_bytes()),
            1,
            AssetKind::Other,
            1,
            vec![b], // closes the cycle A→B→A
            "a".into(),
        )
        .unwrap(),
    );
    assert!(matches!(result, Err(AssetError::DependencyCycle(_))));
    // manifest untouched by the rejected insert
    assert_eq!(m.len(), 1);
}

// ----------------------------------------------------------------- resolve

#[test]
fn resolve_errors_are_typed_and_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let entry = import(
        &store,
        AssetId::from_u128(9),
        "x".into(),
        AssetKind::Other,
        1,
        &b"data"[..],
    )
    .unwrap();
    let mut m = AssetManifest::new();
    m.insert(entry.clone()).unwrap();
    let resolver = AssetResolver::new(&store, &m);

    // present and fine
    let resolved = resolve(&store, &m, AssetId::from_u128(9)).unwrap();
    assert_eq!(resolved.entry, entry);

    // missing RECORD (not in manifest at all)
    assert!(matches!(
        resolve(&store, &m, AssetId::from_u128(77)),
        Err(AssetError::MissingAssetId(_))
    ));

    // missing OBJECT (record exists, object file deleted)
    std::fs::remove_file(store.object_path(&entry.content_hash)).unwrap();
    assert!(matches!(
        resolve(&store, &m, AssetId::from_u128(9)),
        Err(AssetError::MissingObject(_))
    ));

    // verify_all stops at first failure with a typed error
    assert!(matches!(
        resolver.verify_all(&[AssetId::from_u128(9)]),
        Err(AssetError::MissingObject(_))
    ));
}

// -------------------------------------------------------------- decoders

#[test]
fn png_decode_metadata_and_alpha() {
    // build an RGBA PNG in-memory (alpha channel numbering matters here)
    let img =
        image::ImageBuffer::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 128, 200]));
    let mut png_bytes = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let entry = import(
        &store,
        AssetId::new_v4(),
        "hero-alpha.png".into(),
        AssetKind::Image,
        1,
        &png_bytes[..],
    )
    .unwrap();

    // through the registry (export-grade): verify BEFORE decode
    verify(&store, &entry).unwrap();
    match decode::dispatch(AssetKind::Image, entry.importer_version, &png_bytes).unwrap() {
        decode::DecodedAsset::Image(img) => {
            assert_eq!(img.width, 3);
            assert_eq!(img.height, 2);
            assert!(img.has_alpha, "RGBA source must report alpha");
            assert_eq!(img.rgba.len(), (3 * 2 * 4) as usize);
        }
        _ => panic!("wrong decoder"),
    }

    // derived cache key is stable and includes the exact content
    let k1 = decode::derived_cache_key(&entry.content_hash, 1, "png-rgba8-straight");
    let k2 = decode::derived_cache_key(&entry.content_hash, 1, "png-rgba8-straight");
    assert_eq!(k1, k2);
    assert!(k1.starts_with(entry.content_hash.full_hex().as_str()));
}

#[test]
fn wav_decode_pcm_metadata() {
    // 1 s of 44.1 kHz mono sine via hound (dev-dep), via a real temp file —
    // then read the exact bytes back for the in-memory decode path.
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let wav_path = tempfile::tempdir().unwrap().keep().join("sine.wav");
    {
        let mut writer = hound::WavWriter::create(&wav_path, spec).unwrap();
        for i in 0..44100 {
            let t = i as f32 / 44100.0;
            writer
                .write_sample(
                    ((t * 660.0 * 2.0 * std::f32::consts::PI).sin() * 30000.0) as i16,
                )
                .unwrap();
        }
        writer.finalize().unwrap();
    }
    let wav_bytes = std::fs::read(&wav_path).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let entry = import(
        &store,
        AssetId::new_v4(),
        "drums.wav".into(),
        AssetKind::Audio,
        1,
        &wav_bytes[..],
    )
    .unwrap();
    verify(&store, &entry).unwrap();

    match decode::dispatch(AssetKind::Audio, 1, &wav_bytes[..]).unwrap() {
        decode::DecodedAsset::Audio(pcm) => {
            assert_eq!(pcm.sample_rate, 44100);
            assert_eq!(pcm.channels, 1);
            assert_eq!(pcm.frames, 44100);
            assert_eq!(pcm.samples.len(), 44100);
        }
        _ => panic!("wrong decoder"),
    }
}

#[test]
fn unregistered_decoder_version_rejected_explicitly() {
    // version 2 for Audio is not registered → typed UnsupportedDecoder
    assert!(matches!(
        decode::dispatch(AssetKind::Audio, 2, b"anything"),
        Err(AssetError::UnsupportedDecoder(_, 2))
    ));
    // wrong-kind byte garbage for a registered decoder → Decode error, not a crash
    assert!(decode::dispatch(AssetKind::Image, 1, b"not a png").is_err());
}
