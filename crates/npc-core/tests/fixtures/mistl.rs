// Standalone fake CLI compiled by tests. ASCII-only; never invokes real mistl.
use std::{env, fs, io::{self, Read}, path::PathBuf};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    assert_eq!(args[0], "--instance");
    assert_eq!(args[2], "--state-dir");
    assert_eq!(&args[4..6], ["ai", "external"]);
    assert_eq!(&args[7..], ["--owner", "tc-npc"]);
    let dir = PathBuf::from(&args[3]);
    fs::write(dir.join("args.json"), format!("{args:?}")).unwrap();
    match args[1].as_str() {
        "fail" => { eprintln!("error: daemon unavailable"); std::process::exit(2); }
        "invalid" => { println!("not JSON"); return; }
        "slow" => { std::thread::sleep(std::time::Duration::from_secs(30)); return; }
        "normal instance" => {},
        "delayed" => std::thread::sleep(std::time::Duration::from_millis(300)),
        other => panic!("unexpected instance: {other}"),
    }
    let file = dir.join("registration.json");
    match args[6].as_str() {
        "apply" => {
            let mut payload = String::new(); io::stdin().read_to_string(&mut payload).unwrap();
            assert!(payload.contains("\"owner\":\"tc-npc\""));
            fs::write(file, payload).unwrap();
            println!("{{\"owner\":\"tc-npc\",\"applied\":true,\"warnings\":[\"test warning\"]}}");
        }
        "remove" => {
            let removed = fs::remove_file(file).is_ok();
            println!("{{\"owner\":\"tc-npc\",\"removed\":{removed}}}");
        }
        "get" => {
            if let Ok(mut payload) = fs::read_to_string(file) {
                payload = payload.replace("test-secret", "***");
                payload.pop();
                println!("{{\"registrations\":[{payload},\"updated_at\":\"2026-10-04T00:00:00Z\",\"status\":{{\"rooms\":[{{\"room\":\"team\",\"joined\":true,\"providing\":true,\"peers\":2,\"models\":[\"model\"]}}]}}}}]}}");
            } else { println!("{{\"registrations\":[]}}"); }
        }
        other => panic!("unexpected operation: {other}"),
    }
}
