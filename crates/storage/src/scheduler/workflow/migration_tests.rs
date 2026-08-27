use super::*;
use open_compute_core::CronRunId;

#[test]
fn workflow_scheduler_v4_upgrade_preserves_cron_claim_and_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("scheduler.sqlite");
    let connection = create_scheduler_fixture_at_version(&path, 4);
    let activation = CronActivationId::generate();
    let account = AccountId::generate();
    let worker = WorkerId::generate();
    let deployment = DeploymentId::generate();
    let run = CronRunId::generate();
    connection.execute_batch(&format!("INSERT INTO cron_schedules
        (activation_id,account_id,worker_id,deployment_id,execution_generation,activation_generation,
         expression,expression_sha256,parser_version,state,next_fire_at_ms,updated_at_ms)
        VALUES ('{activation}','{account}','{worker}','{deployment}',1,1,'* * * * *',zeroblob(32),1,'accepting',60000,0);
        INSERT INTO cron_runs(id,activation_id,activation_generation,scheduled_at_ms,deployment_id,
          execution_generation,expression,state,next_attempt_at_ms,created_at_ms)
        VALUES ('{run}','{activation}',1,60000,'{deployment}',1,'* * * * *','ready',60000,0);
        UPDATE cron_runs SET state='claimed',next_attempt_at_ms=NULL,claim_token=zeroblob(32),
            claimed_at_ms=60000,claim_until_ms=120000 WHERE id='{run}';")).unwrap();
    drop(connection);
    for now_ms in [60001, 60002] {
        let store = SchedulerStore::open(&path, 1000, now_ms).unwrap();
        assert_eq!(store.inspect_workflows(now_ms).unwrap(), Default::default());
        drop(store);
    }
    let connection = Connection::open(path).unwrap();
    let preserved = connection
        .query_row(
            "SELECT state,claim_until_ms,attempt,length(claim_token) FROM cron_runs WHERE id=?1",
            [run.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(preserved, ("claimed".into(), 120000, 0, 32));
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        5
    );
}
