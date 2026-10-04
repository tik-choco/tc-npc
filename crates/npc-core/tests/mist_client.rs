use npc_core::{
    config::MistConfig,
    mist::{registration, MistClient},
    Config,
};
use std::{path::PathBuf, time::Duration};

fn fixture() -> (PathBuf, MistConfig) {
    let dir = std::env::temp_dir().join(format!("npc fake mistl {}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let executable = dir.join(if cfg!(windows) {
        "fake mistl.exe"
    } else {
        "fake mistl"
    });
    let result = std::process::Command::new("rustc")
        .args(["--edition=2021"])
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mistl.rs"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let config = MistConfig {
        cli_path: executable.to_string_lossy().into_owned(),
        instance: Some("normal instance".into()),
        state_dir: Some(dir.clone()),
        ..Default::default()
    };
    (dir, config)
}

#[tokio::test]
async fn fake_cli_contract_errors_and_timeout() {
    let (dir, mut mist) = fixture();
    let config: Config = serde_json::from_value(serde_json::json!({"providers":[
        {"id":"http","base_url":"http://localhost/v1","api_key":"test-secret"},
        {"id":"r","base_url":"mist-network://team","provide":true,
         "shared":[{"provider_id":"http","model":"model"}]}
    ]}))
    .unwrap();
    let client = MistClient::new(&mist);
    assert_eq!(
        client.apply(&registration(&config)).await.unwrap().warnings,
        ["test warning"]
    );
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("registration.json")).unwrap()).unwrap();
    assert_eq!(stored["providers"][0]["api_key"], "test-secret");
    let result = client.get().await.unwrap();
    assert_eq!(result.registrations[0].status.rooms[0].peers, 2);
    assert_eq!(result.registrations[0].rooms[0].shared[0].model, "model");
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("test-secret"));
    assert!(client.remove().await.unwrap());
    assert!(!client.remove().await.unwrap());
    assert!(client.get().await.unwrap().registrations.is_empty());
    for (mode, expected) in [
        ("fail", "daemon unavailable"),
        ("invalid", "invalid external API JSON"),
        ("slow", "timed out"),
    ] {
        mist.instance = Some(mode.into());
        let error = MistClient::new(&mist)
            .with_timeout(Duration::from_millis(500))
            .get()
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
    mist.cli_path = dir.join("missing.exe").to_string_lossy().into_owned();
    assert!(MistClient::new(&mist)
        .get()
        .await
        .unwrap_err()
        .to_string()
        .contains("could not start mistl"));
    // Windows may still hold the image briefly while Tokio reaps a timed-out child.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    tokio::time::sleep(Duration::from_millis(20)).await
                }
                Err(error) => panic!("fixture cleanup failed: {error}"),
            }
        }
    })
    .await
    .unwrap();
}
