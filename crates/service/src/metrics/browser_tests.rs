use super::*;
use open_compute_core::{MetricsConfig, PlatformStatus};

#[test]
fn browser_metrics_bound_labels_classify_results_and_finish_cancelled_operations() {
    let registry =
        Arc::new(MetricsRegistry::new(&MetricsConfig::default(), "test", "workerd").unwrap());
    for operation in BrowserOperation::ALL {
        for outcome in BrowserOutcome::ALL {
            let guard = registry.browser_operation(operation);
            let live = registry.render(&PlatformStatus::starting());
            assert!(live.contains(&format!(
                "browser_in_flight_operations{{operation=\"{}\"}} 1",
                operation.as_str()
            )));
            if matches!(outcome, BrowserOutcome::Cancelled) {
                drop(guard);
            } else {
                guard.finish(outcome);
            }
        }
    }
    for (error, expected) in [
        (ErrorCode::BrowserLimitExceeded, BrowserOutcome::Limit),
        (ErrorCode::BrowserTimeout, BrowserOutcome::Timeout),
        (ErrorCode::BrowserInputInvalid, BrowserOutcome::Failure),
    ] {
        let result: Result<(), PlatformError> = Err(PlatformError::new(error, "credential-canary"));
        assert_eq!(BrowserOutcome::result(&result) as usize, expected as usize);
    }
    assert!(matches!(
        BrowserOutcome::result(&Ok::<_, PlatformError>(())),
        BrowserOutcome::Success
    ));
    for (status, outcome) in [
        (200, BrowserOutcome::Success),
        (400, BrowserOutcome::Failure),
        (413, BrowserOutcome::Limit),
        (429, BrowserOutcome::Limit),
        (504, BrowserOutcome::Timeout),
    ] {
        assert_eq!(
            BrowserOutcome::status(axum::http::StatusCode::from_u16(status).unwrap()) as usize,
            outcome as usize
        );
    }
    for name in [
        "content",
        "screenshot",
        "pdf",
        "snapshot",
        "scrape",
        "links",
        "markdown",
        "json",
        "accessibilityTree",
    ] {
        assert!(BrowserOperation::action(name).is_some());
    }
    assert!(BrowserOperation::action("credential-canary").is_none());
    registry.set_browser_sessions(2);
    let text = registry.render(&PlatformStatus::starting());
    assert!(text.contains("browser_active_sessions 2"));
    for operation in BrowserOperation::ALL {
        let label = operation.as_str();
        for outcome in BrowserOutcome::ALL {
            assert!(text.contains(&format!(
                "browser_operations_total{{operation=\"{label}\",outcome=\"{}\"}} 1",
                outcome.as_str()
            )));
        }
        assert!(text.contains(&format!(
            "browser_in_flight_operations{{operation=\"{label}\"}} 0"
        )));
        assert!(text.contains(&format!(
            "browser_operation_duration_seconds_count{{operation=\"{label}\"}} 5"
        )));
        let sum = text
            .lines()
            .find_map(|line| {
                line.strip_prefix(&format!(
                    "browser_operation_duration_seconds_sum{{operation=\"{label}\"}} "
                ))
            })
            .unwrap()
            .parse::<f64>()
            .unwrap();
        assert!(sum.is_finite() && sum >= 0.0);
    }
    assert!(!text.contains("credential-canary"));
    assert_eq!(
        text.lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .count() as u64,
        super::super::REQUIRED_SERIES
    );
}
