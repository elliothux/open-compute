use super::*;

#[tokio::test]
async fn preflight_fails_closed_on_each_stage_and_cleans_up() {
    expect_preflight_fail(Fault::Auth).await;
    expect_preflight_fail(Fault::Permission).await;
    expect_preflight_fail(Fault::ServerError).await;
    expect_preflight_fail(Fault::NotFound).await;
    expect_preflight_fail(Fault::DeleteFail).await;
    expect_preflight_fail(Fault::CorruptMetadata).await;
    expect_preflight_fail(Fault::CorruptBody).await;
}
