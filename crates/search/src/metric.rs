use crate::{SearchError, validate_vector};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Supported Vectorize distance metric.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DistanceMetric {
    /// Raw cosine similarity in the public negative-one-to-one range; larger is closer.
    Cosine,
    /// Raw Euclidean L2 distance; smaller is closer.
    Euclidean,
    /// Raw dot product similarity; larger is closer.
    DotProduct,
}

impl DistanceMetric {
    /// Stable configuration token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cosine => "cosine",
            Self::Euclidean => "euclidean",
            Self::DotProduct => "dot-product",
        }
    }
}

impl FromStr for DistanceMetric {
    type Err = SearchError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cosine" => Ok(Self::Cosine),
            "euclidean" => Ok(Self::Euclidean),
            "dot-product" => Ok(Self::DotProduct),
            _ => Err(SearchError::InvalidMetric),
        }
    }
}

/// Query vector validated once before an exact scan.
#[derive(Clone, Debug)]
pub struct PreparedQuery {
    metric: DistanceMetric,
    values: Vec<f32>,
    norm: f64,
}

impl PreparedQuery {
    /// Validate and prepare a query for repeated candidate scoring.
    pub fn new(
        metric: DistanceMetric,
        values: &[f32],
        dimensions: usize,
    ) -> Result<Self, SearchError> {
        validate_vector(values, dimensions)?;
        let norm = if metric == DistanceMetric::Cosine {
            squared_sum(values).sqrt()
        } else {
            0.0
        };
        Ok(Self {
            metric,
            values: values.to_vec(),
            norm,
        })
    }

    /// Number of vector components required for every candidate.
    #[must_use]
    pub fn dimensions(&self) -> usize {
        self.values.len()
    }

    /// Score a validated candidate and return the public similarity score.
    pub fn score(&self, candidate: &[f32]) -> Result<f64, SearchError> {
        validate_vector(candidate, self.values.len())?;
        let raw = match self.metric {
            DistanceMetric::Cosine => {
                let candidate_norm = squared_sum(candidate).sqrt();
                if self.norm == 0.0 || candidate_norm == 0.0 {
                    -1.0
                } else {
                    dot(&self.values, candidate) / (self.norm * candidate_norm)
                }
            }
            DistanceMetric::Euclidean => squared_distance(&self.values, candidate).sqrt(),
            DistanceMetric::DotProduct => dot(&self.values, candidate),
        };
        Ok(normalize_public_score(self.metric, raw))
    }
}

/// Compute the metric's raw similarity or distance using f64 accumulation.
pub fn raw_score(
    metric: DistanceMetric,
    query: &[f32],
    candidate: &[f32],
) -> Result<f64, SearchError> {
    let prepared = PreparedQuery::new(metric, query, query.len())?;
    validate_vector(candidate, query.len())?;
    match metric {
        DistanceMetric::Cosine => {
            let candidate_norm = squared_sum(candidate).sqrt();
            if prepared.norm == 0.0 || candidate_norm == 0.0 {
                Ok(-1.0)
            } else {
                Ok(dot(query, candidate) / (prepared.norm * candidate_norm))
            }
        }
        DistanceMetric::Euclidean => Ok(squared_distance(query, candidate).sqrt()),
        DistanceMetric::DotProduct => Ok(dot(query, candidate)),
    }
}

/// Clamp only cosine round-off while preserving the public raw distance score.
#[must_use]
pub fn normalize_public_score(metric: DistanceMetric, raw: f64) -> f64 {
    match metric {
        DistanceMetric::Cosine => raw.clamp(-1.0, 1.0),
        DistanceMetric::Euclidean | DistanceMetric::DotProduct => raw,
    }
}

impl DistanceMetric {
    /// Compare two public scores according to the metric's Cloudflare ordering.
    #[must_use]
    pub fn compare_scores(self, left: f64, right: f64) -> std::cmp::Ordering {
        match self {
            Self::Cosine | Self::DotProduct => left.total_cmp(&right),
            Self::Euclidean => right.total_cmp(&left),
        }
    }
}

fn dot(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| f64::from(*left) * f64::from(*right))
        .sum()
}

fn squared_sum(values: &[f32]) -> f64 {
    values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum()
}

fn squared_distance(left: &[f32], right: &[f32]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| {
            let delta = f64::from(*left) - f64::from(*right);
            delta * delta
        })
        .sum()
}
