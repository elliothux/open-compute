//! Fixed Playwright download and shared session metadata behavior in real Workers.
use super::*;

pub(super) fn check(bytes: &[u8]) {
    let result: Value = serde_json::from_slice(bytes).unwrap();
    let metadata = &result["sessionMetadata"];
    assert_eq!(metadata["puppeteer"], metadata["playwright"]);
    let active = metadata["puppeteer"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["connectionId"].is_string())
        .unwrap();
    assert!(active["startTime"].is_number());
    assert!(
        active["connectionStartTime"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            >= active["startTime"].as_i64().unwrap()
    );
    let downloads = result["downloads"].as_array().unwrap();
    assert_eq!(downloads.len(), 3);
    for (index, download) in downloads.iter().enumerate() {
        assert_eq!(download["filename"], "fixture.txt");
        if index == 2 {
            assert!(
                download["failure"]
                    .as_str()
                    .unwrap()
                    .contains("acceptDownloads"),
                "{download}"
            );
        } else {
            assert!(download["failure"].is_null(), "{download}");
            assert_eq!(download["virtualPath"], true);
            for error in ["pathError", "saveError"] {
                assert!(
                    download[error]
                        .as_str()
                        .unwrap()
                        .contains("no such file or directory"),
                    "{download}"
                );
            }
            assert_eq!(download["streamBytes"], "", "{download}");
        }
    }
    for text in ["/private/", "control.sqlite", "master.key"] {
        assert!(!String::from_utf8_lossy(bytes).contains(text));
    }
}
