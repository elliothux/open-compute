use super::*;

fn words(text: &str) -> usize {
    text.split_whitespace().count()
}

#[test]
fn chunks_prefer_boundaries_and_preserve_utf8_offsets() {
    let input = "一 二 三。 four five six seven";
    let chunks = chunk_text(
        input,
        ChunkConfig {
            max_tokens: 4,
            overlap_tokens: 1,
        },
        words,
    )
    .expect("chunk");
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].text.trim_end(), "一 二 三。");
    assert_eq!(
        &input[chunks[1].start_byte..chunks[1].end_byte],
        chunks[1].text
    );
    assert!(chunks.iter().all(|chunk| words(&chunk.text) <= 4));
}

#[test]
fn overlap_always_makes_strict_progress() {
    let input = "one two\n\nthree four five six seven eight nine ten eleven";
    let chunks = chunk_text(
        input,
        ChunkConfig {
            max_tokens: 10,
            overlap_tokens: 3,
        },
        words,
    )
    .expect("chunk");
    assert!(chunks.len() >= 2);
    assert!(
        chunks
            .windows(2)
            .all(|pair| pair[1].start_byte > pair[0].start_byte)
    );
}

#[test]
fn chunk_contract_rejects_invalid_config_and_tokenizer_behavior() {
    assert_eq!(
        ChunkConfig {
            max_tokens: 0,
            overlap_tokens: 0,
        }
        .validate(),
        Err(ChunkError::InvalidConfig)
    );
    assert_eq!(
        ChunkConfig {
            max_tokens: 10,
            overlap_tokens: 4,
        }
        .validate(),
        Err(ChunkError::InvalidConfig)
    );
    assert_eq!(
        chunk_text(
            "nonempty",
            ChunkConfig {
                max_tokens: 2,
                overlap_tokens: 0,
            },
            |_| 0,
        ),
        Err(ChunkError::InvalidTokenizer)
    );
    assert!(
        chunk_text(
            "  \n\t",
            ChunkConfig {
                max_tokens: 2,
                overlap_tokens: 0,
            },
            |_| 1,
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        ChunkError::InvalidConfig.to_string(),
        "chunk configuration is invalid"
    );
    assert_eq!(
        ChunkError::InvalidTokenizer.to_string(),
        "tokenizer contract is invalid"
    );
}

#[test]
fn hostile_fts_input_becomes_literals_only() {
    let query = build_fts_query("title:secret OR foo* NEAR(bar)", KeywordMatchMode::And, 10)
        .expect("query");
    assert_eq!(
        query,
        "\"title\" AND \"secret\" AND \"or\" AND \"foo\" AND \"near\" AND \"bar\""
    );
}

#[test]
fn fts_query_supports_or_and_rejects_empty_or_excess_terms() {
    assert_eq!(
        build_fts_query("Rust SQLITE", KeywordMatchMode::Or, 2).unwrap(),
        "\"rust\" OR \"sqlite\""
    );
    assert_eq!(
        build_fts_query("***", KeywordMatchMode::And, 1),
        Err(KeywordQueryError::Empty)
    );
    assert_eq!(
        build_fts_query("one two", KeywordMatchMode::And, 1),
        Err(KeywordQueryError::TooManyTerms)
    );
    assert_eq!(
        KeywordQueryError::Empty.to_string(),
        "keyword query is empty"
    );
    assert_eq!(
        KeywordQueryError::TooManyTerms.to_string(),
        "keyword query has too many terms"
    );
}

#[test]
fn cosine_and_hybrid_order_are_deterministic() {
    assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Ok(1.0));
    let vector = vec![
        RankedCandidate {
            chunk_id: "b".into(),
            score: 0.9,
        },
        RankedCandidate {
            chunk_id: "a".into(),
            score: 0.8,
        },
    ];
    let keyword = vec![
        RankedCandidate {
            chunk_id: "a".into(),
            score: 0.7,
        },
        RankedCandidate {
            chunk_id: "b".into(),
            score: 0.6,
        },
    ];
    let result =
        fuse_candidates(&vector, &keyword, FusionMethod::ReciprocalRank, 10, 0.0).expect("fusion");
    assert_eq!(result[0].chunk_id, "a");
    assert_eq!(result[1].chunk_id, "b");
    assert!(result.iter().all(|candidate| candidate.score > 0.9));

    let single = fuse_candidates(&vector, &[], FusionMethod::ReciprocalRank, 10, 0.4)
        .expect("single branch");
    assert_eq!(single.len(), 2);
    assert!(single[0].score > 0.49 && single[0].score < 0.51);

    let maximum = fuse_candidates(&vector, &keyword, FusionMethod::Maximum, 1, 0.85).unwrap();
    assert_eq!(maximum.len(), 1);
    assert_eq!(maximum[0].chunk_id, "b");
    assert_eq!(maximum[0].vector_rank, Some(1));
    assert_eq!(maximum[0].keyword_rank, Some(2));
}

#[test]
fn invalid_retrieval_values_fail_closed() {
    assert_eq!(
        cosine_similarity(&[], &[]),
        Err(RetrievalError::DimensionMismatch)
    );
    assert_eq!(
        cosine_similarity(&[1.0], &[1.0, 2.0]),
        Err(RetrievalError::DimensionMismatch)
    );
    assert_eq!(
        cosine_similarity(&[f32::NAN], &[1.0]),
        Err(RetrievalError::NonFiniteValue)
    );
    assert_eq!(
        cosine_similarity(&[0.0], &[0.0]),
        Err(RetrievalError::ZeroNorm)
    );
    let duplicate = vec![
        RankedCandidate {
            chunk_id: "x".into(),
            score: 1.0,
        },
        RankedCandidate {
            chunk_id: "x".into(),
            score: 0.5,
        },
    ];
    assert_eq!(
        fuse_candidates(&duplicate, &[], FusionMethod::Maximum, 1, 0.0),
        Err(RetrievalError::DuplicateCandidate)
    );
    assert_eq!(
        fuse_candidates(&[], &[], FusionMethod::Maximum, 0, 0.0),
        Err(RetrievalError::InvalidLimit)
    );
    assert_eq!(
        fuse_candidates(&[], &[], FusionMethod::Maximum, 51, 0.0),
        Err(RetrievalError::InvalidLimit)
    );
    for threshold in [f32::NAN, -0.1, 1.1] {
        assert_eq!(
            fuse_candidates(&[], &[], FusionMethod::Maximum, 1, threshold),
            Err(RetrievalError::NonFiniteValue)
        );
    }
    assert_eq!(
        fuse_candidates(
            &[RankedCandidate {
                chunk_id: "x".into(),
                score: 2.0,
            }],
            &[],
            FusionMethod::Maximum,
            1,
            0.0,
        ),
        Err(RetrievalError::NonFiniteValue)
    );
    for (error, message) in [
        (
            RetrievalError::DimensionMismatch,
            "vector dimensions do not match",
        ),
        (
            RetrievalError::NonFiniteValue,
            "retrieval input is not finite",
        ),
        (RetrievalError::ZeroNorm, "cosine vector has zero norm"),
        (
            RetrievalError::DuplicateCandidate,
            "retrieval branch has a duplicate",
        ),
        (
            RetrievalError::InvalidLimit,
            "retrieval result limit is invalid",
        ),
    ] {
        assert_eq!(error.to_string(), message);
    }
}
