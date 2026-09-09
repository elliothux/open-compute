use super::*;

pub(super) async fn exercise_kv(command: &WranglerCommand<'_>, project: &Path) {
    assert_success(
        &command
            .run(&[
                "kv",
                "namespace",
                "create",
                KV_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let listed = command
        .run(&["kv", "namespace", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let namespaces = json_stdout(&listed);
    let namespace_id = namespaces
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["title"] == KV_NAME)
        .and_then(|item| item["id"].as_str())
        .unwrap()
        .to_owned();

    write_config(project, command.account_id, Some(&namespace_id), None);
    for args in [
        vec![
            "kv",
            "key",
            "put",
            "greeting",
            "你好 🌍",
            "--namespace-id",
            &namespace_id,
            "--remote",
            "--config",
            "wrangler.jsonc",
        ],
        vec![
            "kv",
            "key",
            "list",
            "--namespace-id",
            &namespace_id,
            "--remote",
            "--config",
            "wrangler.jsonc",
        ],
    ] {
        assert_success(&command.run(&args).await);
    }
    let value = command
        .run(&[
            "kv",
            "key",
            "get",
            "greeting",
            "--namespace-id",
            &namespace_id,
            "--remote",
            "--text",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&value);
    assert_eq!(String::from_utf8_lossy(&value.stdout).trim(), "你好 🌍");
    assert_success(
        &command
            .run(&[
                "kv",
                "key",
                "delete",
                "greeting",
                "--namespace-id",
                &namespace_id,
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "kv",
                "namespace",
                "delete",
                "--namespace-id",
                &namespace_id,
                "--skip-confirmation",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

pub(super) async fn exercise_d1(command: &WranglerCommand<'_>, project: &Path) {
    assert_success(
        &command
            .run(&["d1", "create", D1_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let listed = command
        .run(&["d1", "list", "--json", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    let databases = json_stdout(&listed);
    let database_id = databases
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == D1_NAME)
        .and_then(|item| item["uuid"].as_str())
        .unwrap()
        .to_owned();
    write_config(project, command.account_id, None, Some(&database_id));
    fs::create_dir(project.join("migrations")).unwrap();
    fs::write(
        project.join("migrations/0001_create_items.sql"),
        "CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL);",
    )
    .unwrap();

    let info = command
        .run(&[
            "d1",
            "info",
            D1_NAME,
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&info);
    assert!(json_contains(
        &json_stdout(&info),
        "name",
        &Value::from(D1_NAME)
    ));

    let answer = command
        .run(&[
            "d1",
            "execute",
            D1_NAME,
            "--remote",
            "--command",
            "SELECT 42 AS answer",
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&answer);
    assert!(json_contains(
        &json_stdout(&answer),
        "answer",
        &Value::from(42)
    ));

    assert_success(
        &command
            .run(&[
                "d1",
                "migrations",
                "apply",
                D1_NAME,
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let migrated = command
        .run(&[
            "d1",
            "execute",
            D1_NAME,
            "--remote",
            "--command",
            "SELECT name FROM sqlite_master WHERE type='table' AND name='items'",
            "--json",
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&migrated);
    assert!(json_contains(
        &json_stdout(&migrated),
        "name",
        &Value::from("items")
    ));
    assert_success(
        &command
            .run(&[
                "d1",
                "delete",
                D1_NAME,
                "--skip-confirmation",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let after_delete = command
        .run(&["d1", "list", "--json", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&after_delete);
    assert!(!json_contains(
        &json_stdout(&after_delete),
        "name",
        &Value::from(D1_NAME)
    ));
}

pub(super) async fn exercise_r2(command: &WranglerCommand<'_>, project: &Path) {
    assert_success(
        &command
            .run(&[
                "r2",
                "bucket",
                "create",
                R2_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let listed = command
        .run(&["r2", "bucket", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(R2_NAME));
    fs::write(project.join("r2-input.bin"), b"fixed-wrangler-r2\0payload").unwrap();
    assert_success(
        &command
            .run(&[
                "r2",
                "object",
                "put",
                &format!("{R2_NAME}/folder/object.bin"),
                "--file",
                "r2-input.bin",
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "r2",
                "object",
                "get",
                &format!("{R2_NAME}/folder/object.bin"),
                "--file",
                "r2-output.bin",
                "--remote",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_eq!(
        fs::read(project.join("r2-output.bin")).unwrap(),
        b"fixed-wrangler-r2\0payload"
    );
    assert_success(
        &command
            .run(&[
                "r2",
                "object",
                "delete",
                &format!("{R2_NAME}/folder/object.bin"),
                "--remote",
                "--force",
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    assert_success(
        &command
            .run(&[
                "r2",
                "bucket",
                "delete",
                R2_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
}

pub(super) async fn exercise_queues(command: &WranglerCommand<'_>) {
    assert_success(
        &command
            .run(&["queues", "create", QUEUE_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let listed = command
        .run(&["queues", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(QUEUE_NAME));
    assert_success(
        &command
            .run(&["queues", "delete", QUEUE_NAME, "--config", "wrangler.jsonc"])
            .await,
    );
    let after_delete = command
        .run(&["queues", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&after_delete);
    assert!(!String::from_utf8_lossy(&after_delete.stdout).contains(QUEUE_NAME));
}

pub(super) async fn exercise_workflows(command: &WranglerCommand<'_>) {
    let listed = command
        .run(&["workflows", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&listed);
    assert!(String::from_utf8_lossy(&listed.stdout).contains(WORKFLOW_NAME));
    let described = command
        .run(&[
            "workflows",
            "describe",
            WORKFLOW_NAME,
            "--config",
            "wrangler.jsonc",
        ])
        .await;
    assert_success(&described);
    let described = String::from_utf8_lossy(&described.stdout);
    assert!(described.contains(WORKFLOW_NAME));
    assert!(described.contains("resource-gate-worker"));
    assert!(described.contains("ResourceFlow"));
    assert_success(
        &command
            .run(&[
                "workflows",
                "delete",
                WORKFLOW_NAME,
                "--config",
                "wrangler.jsonc",
            ])
            .await,
    );
    let after_delete = command
        .run(&["workflows", "list", "--config", "wrangler.jsonc"])
        .await;
    assert_success(&after_delete);
    assert!(!String::from_utf8_lossy(&after_delete.stdout).contains(WORKFLOW_NAME));
}
