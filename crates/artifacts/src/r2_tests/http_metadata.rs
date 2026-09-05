use super::*;

#[test]
fn http_metadata_preserves_absence_and_rejects_lost_declared_headers() {
    let provider = crate::ObjectHttpMetadata {
        content_type: Some("application/octet-stream".to_owned()),
        content_language: Some("en".to_owned()),
        content_disposition: Some("attachment".to_owned()),
        content_encoding: Some("identity".to_owned()),
        cache_control: Some("max-age=60".to_owned()),
        cache_expiry: Some(1_000),
    };
    for fields in 0..64 {
        let declared = R2HttpMetadata {
            content_type: (fields & 1 != 0)
                .then(|| provider.content_type.clone())
                .flatten(),
            content_language: (fields & 2 != 0)
                .then(|| provider.content_language.clone())
                .flatten(),
            content_disposition: (fields & 4 != 0)
                .then(|| provider.content_disposition.clone())
                .flatten(),
            content_encoding: (fields & 8 != 0)
                .then(|| provider.content_encoding.clone())
                .flatten(),
            cache_control: (fields & 16 != 0)
                .then(|| provider.cache_control.clone())
                .flatten(),
            cache_expiry: (fields & 32 != 0)
                .then_some(provider.cache_expiry)
                .flatten(),
        };
        let mut object = ObjectMetadata {
            user: create_user_metadata(
                &uuid::Uuid::now_v7().to_string(),
                &BTreeMap::new(),
                &declared,
                R2StorageClass::Standard,
                None,
            )
            .unwrap(),
            http: provider.clone(),
            etag: "etag".to_owned(),
            ..ObjectMetadata::default()
        };
        assert_eq!(
            decode_metadata("key", &object, None).unwrap().http_metadata,
            Some(declared)
        );
        if fields != 0 {
            object.http = crate::ObjectHttpMetadata::default();
            assert!(decode_metadata("key", &object, None).is_err());
        }
        for invalid in ["", "01", "64", "-1", "256", "invalid"] {
            object
                .user
                .insert(META_HTTP_FIELDS.to_owned(), invalid.to_owned());
            assert!(decode_metadata("key", &object, None).is_err());
        }
        object.user.remove(META_HTTP_FIELDS);
        assert!(
            decode_metadata("key", &object, None).is_err(),
            "missing authority is not backfilled"
        );
    }
}
