// Standard Tauri build script: embeds the Windows .ico resource into the
// exe and compiles the permissions ACL from tauri.conf.json + capabilities/
// into the binary. Only runs for a `--features desktop` build -- for a
// headless build `tauri-build` isn't even a compiled dependency (see the
// `desktop` feature in Cargo.toml), so the call has to stay behind the same
// `cfg` or this wouldn't compile at all without the feature.
fn main() {
    #[cfg(feature = "desktop")]
    tauri_build::build();
}
