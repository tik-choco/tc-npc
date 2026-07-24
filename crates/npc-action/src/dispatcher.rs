//! The text command dispatcher (ports Go `internal/cli/*.go`'s command
//! table plus `ExecuteActions`/`stopActiveTask`). `execute_command` handles
//! one line of CLI-style input; `execute_actions` runs an ordered list of
//! `ActionItem`s (from either a single dispatched command or an NL
//! action-chat response) as a single cancellable background sequence,
//! cancelling whatever sequence was previously running first — exactly the
//! "starting a new one cancels the previous" semantics of the Go CLI.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::state::{ActionState, ActiveTask};
use crate::types::ActionItem;

/// Ports the Go CLI's `isImmediate` allowlist (`stop`, `pos`, `reset`,
/// `locations`, `routes`, plus `cal`/`mcal`, which this port answers
/// immediately with an "unavailable" notice instead of running Go's
/// interactive calibration wizard).
const IMMEDIATE_COMMANDS: &[&str] = &["stop", "pos", "reset", "locations", "routes", "cal", "mcal"];

/// Every other registered command name — dispatched as a single-item
/// cancellable action sequence (ports `CLI.dispatch`'s non-immediate path).
const SEQUENCE_COMMANDS: &[&str] = &[
    "w", "s", "a", "d", "fw", "tr", "tl", "jump", "mv", "mb", "rt", "lt", "mvt", "rtt", "ltt",
    "goto", "home", "go", "route",
];

fn is_known_command(name: &str) -> bool {
    IMMEDIATE_COMMANDS.contains(&name) || SEQUENCE_COMMANDS.contains(&name)
}

/// Ports `CLI.Run`'s per-line dispatch: parse the first whitespace-separated
/// token as a command name; if it isn't one of ours, treat the whole line
/// as natural language and hand it to the LLM action-chat flow.
pub async fn execute_command(state: Arc<ActionState>, text: &str) {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let Some(first) = parts.first() else { return };
    let cmd = first.to_lowercase();

    if !is_known_command(&cmd) {
        let state = state.clone();
        let text = text.to_string();
        tokio::spawn(async move {
            crate::llm_action::chat_with_action(state, text).await;
        });
        return;
    }

    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    if IMMEDIATE_COMMANDS.contains(&cmd.as_str()) {
        run_immediate(&state, &cmd).await;
    } else {
        execute_actions(state, vec![ActionItem { name: cmd, args }]).await;
    }
}

async fn run_immediate(state: &Arc<ActionState>, cmd: &str) {
    match cmd {
        "stop" => {
            stop_active_task(state).await;
            let _ = state.vrc.vertical(0.0).await;
            let _ = state.vrc.horizontal(0.0).await;
            let _ = state.vrc.look_horizontal(0.0).await;
            state.log("Stopping movements...");
        }
        "pos" => state.log(format!("Position: {}", state.navigator.pos.get().display())),
        "reset" => {
            state.navigator.pos.reset();
            state.navigator.publish_position();
            state.log("Position reset to origin");
        }
        "locations" => state.log(format_locations(state)),
        "routes" => state.log(format_routes(state)),
        "cal" | "mcal" => state.log(calibration_unavailable_message()),
        other => unreachable!("run_immediate called with non-immediate command '{other}'"),
    }
}

fn calibration_unavailable_message() -> &'static str {
    "キャリブレーションコマンド(cal/mcal)はこのRust移植版では利用できません。\
     速度モデル定数は固定値を使用しています。"
}

fn format_locations(state: &ActionState) -> String {
    let locations = &state.config.action.locations;
    if locations.is_empty() {
        return "No locations configured".to_string();
    }
    let mut lines = vec![format!("{:<15} {:<8} {:<8} {:<10}", "Name", "X", "Y", "Heading")];
    lines.push("-------------------------------------------".to_string());
    for loc in locations {
        lines.push(format!(
            "{:<15} {:<8.1} {:<8.1} {:<10.1}\u{b0}",
            loc.name, loc.x, loc.y, loc.heading
        ));
    }
    lines.join("\n")
}

fn format_routes(state: &ActionState) -> String {
    let routes = &state.config.action.routes;
    if routes.is_empty() {
        return "No routes configured".to_string();
    }
    routes
        .iter()
        .map(|r| {
            let loop_str = if r.r#loop { " [loop]" } else { "" };
            let waypoints: Vec<&str> = r.waypoints.iter().map(|w| w.location.as_str()).collect();
            format!("{:<15} {}{}", r.name, waypoints.join(" -> "), loop_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ports `CLI.stopActiveTask`: cancels and awaits whatever action sequence
/// is currently running, if any.
pub async fn stop_active_task(state: &Arc<ActionState>) {
    let task = state.active.lock().unwrap().take();
    if let Some(task) = task {
        task.token.cancel();
        let _ = task.handle.await;
        state.log("Background task stopped");
    }
}

/// Ports `CLI.ExecuteActions`: cancels any previous sequence, then runs
/// `items` in order as a new cancellable background task, checking
/// cancellation between each item.
pub async fn execute_actions(state: Arc<ActionState>, items: Vec<ActionItem>) {
    stop_active_task(&state).await;

    let token = state.shutdown.child_token();
    let run_state = state.clone();
    let run_token = token.clone();
    let handle = tokio::spawn(async move {
        for item in items {
            if run_token.is_cancelled() {
                run_state.log("[Action] Sequence cancelled");
                return;
            }
            run_state.log(format!("[Action] Executing: {} {:?}", item.name, item.args));
            run_command(&run_state, &item.name, &item.args, &run_token).await;
        }
        run_state.log("[Action] Task sequence completed");
    });

    *state.active.lock().unwrap() = Some(ActiveTask { token, handle });
}

/// Ports `CLI.runAction`: dispatches a single named command with its args.
/// Reachable both from `execute_actions` (one command, or a whole NL action
/// list) and, via `route`, from `Autopilot::run_route`'s per-waypoint work.
/// Unlike Go's `runAction`, this treats `stop`-as-an-action specially
/// (zeroing axes without touching the active-task slot) since we're
/// already running inside that very task here — routing it through
/// `stop_active_task` (as the top-level `stop` command does) would await
/// this task's own `JoinHandle` and hang forever. Go's equivalent path has
/// the same latent self-deadlock; this port simply avoids it.
async fn run_command(state: &Arc<ActionState>, name: &str, args: &[String], cancel: &CancellationToken) {
    let name = name.to_lowercase();
    let result: anyhow::Result<()> = async {
        match name.as_str() {
            "w" => state.vrc.vertical(1.0).await?,
            "s" => state.vrc.vertical(-1.0).await?,
            "a" => state.vrc.horizontal(-1.0).await?,
            "d" => state.vrc.horizontal(1.0).await?,
            "fw" => {
                let dur = parse_f64(args, 0, 2.0);
                state.controller.move_forward(dur, cancel).await?;
            }
            "tr" => {
                let dur = parse_f64(args, 0, 1.0);
                state.controller.turn(1.0, dur, cancel).await?;
            }
            "tl" => {
                let dur = parse_f64(args, 0, 1.0);
                state.controller.turn(-1.0, dur, cancel).await?;
            }
            "jump" => {
                state.controller.jump(cancel).await?;
            }
            "mv" => {
                let meters = parse_f64(args, 0, 1.0);
                state.navigator.move_meters(meters, cancel).await?;
            }
            "mb" => {
                let meters = parse_f64(args, 0, 1.0);
                state.navigator.move_meters(-meters, cancel).await?;
            }
            "rt" => {
                let deg = parse_f64(args, 0, 90.0);
                state.navigator.turn_degrees(deg, cancel).await?;
            }
            "lt" => {
                let deg = parse_f64(args, 0, 90.0);
                state.navigator.turn_degrees(-deg, cancel).await?;
            }
            "mvt" => {
                let meters = parse_f64(args, 0, 2.0);
                let sec = parse_f64(args, 1, 4.0);
                state.navigator.move_meters_in_time(meters, sec, cancel).await?;
            }
            "rtt" => {
                let deg = parse_f64(args, 0, 180.0);
                let sec = parse_f64(args, 1, 3.0);
                state.navigator.turn_degrees_in_time(deg, sec, cancel).await?;
            }
            "ltt" => {
                let deg = parse_f64(args, 0, 180.0);
                let sec = parse_f64(args, 1, 3.0);
                state.navigator.turn_degrees_in_time(-deg, sec, cancel).await?;
            }
            "goto" => {
                if args.len() < 2 {
                    state.log("Usage: goto <x> <y> [heading] [seconds]");
                } else {
                    let x = parse_f64(args, 0, 0.0);
                    let y = parse_f64(args, 1, 0.0);
                    let sec = parse_f64(args, 3, 10.0);
                    if args.len() >= 3 {
                        let heading = parse_f64(args, 2, 0.0);
                        state.autopilot.go_to_with_heading(x, y, heading, sec, cancel).await?;
                    } else {
                        state.autopilot.go_to(x, y, sec, cancel).await?;
                    }
                    state.log(format!("Position: {}", state.navigator.pos.get().display()));
                }
            }
            "home" => {
                let sec = parse_f64(args, 0, 10.0);
                state.autopilot.return_to_origin(sec, cancel).await?;
                state.log(format!("Position: {}", state.navigator.pos.get().display()));
            }
            "go" => {
                if args.is_empty() {
                    state.log("Usage: go <location_name> [seconds]");
                } else {
                    let target = &args[0];
                    let sec = parse_f64(args, 1, 10.0);
                    let loc = state
                        .config
                        .action
                        .locations
                        .iter()
                        .find(|l| l.name.to_lowercase() == target.to_lowercase())
                        .cloned();
                    match loc {
                        Some(loc) => {
                            state
                                .autopilot
                                .go_to_with_heading(loc.x, loc.y, loc.heading, sec, cancel)
                                .await?;
                            state.log(format!("Position: {}", state.navigator.pos.get().display()));
                        }
                        None => {
                            state.log(format!("Location '{target}' not found. Use 'locations' to list."))
                        }
                    }
                }
            }
            "route" => {
                if args.is_empty() {
                    state.log("Usage: route <route_name>");
                } else {
                    let target = &args[0];
                    let route = state
                        .config
                        .action
                        .routes
                        .iter()
                        .find(|r| r.name.to_lowercase() == target.to_lowercase())
                        .cloned();
                    match route {
                        Some(route) => {
                            let locations = state.config.action.locations.clone();
                            let log_state = state.clone();
                            if let Err(e) = state
                                .autopilot
                                .run_route(&route, &locations, cancel, move |line| log_state.log(line))
                                .await
                            {
                                state.log(format!("[Route] Error: {e}"));
                            }
                        }
                        None => state.log(format!("Route '{target}' not found. Use 'routes' to list.")),
                    }
                }
            }
            // The immediate commands are also reachable here when they show
            // up inside an action sequence (e.g. the LLM emits "pos" as one
            // of its actions) — Go's `runAction` doesn't distinguish either.
            "pos" => state.log(format!("Position: {}", state.navigator.pos.get().display())),
            "reset" => {
                state.navigator.pos.reset();
                state.navigator.publish_position();
                state.log("Position reset to origin");
            }
            "locations" => state.log(format_locations(state)),
            "routes" => state.log(format_routes(state)),
            "cal" | "mcal" => state.log(calibration_unavailable_message()),
            "stop" => {
                let _ = state.vrc.vertical(0.0).await;
                let _ = state.vrc.horizontal(0.0).await;
                let _ = state.vrc.look_horizontal(0.0).await;
                state.log("Stopping movements...");
            }
            other => state.log(format!("Unknown action '{other}'")),
        }
        Ok(())
    }
    .await;

    if let Err(e) = result {
        state.log(format!("Error executing '{name}': {e}"));
    }
}

fn parse_f64(args: &[String], index: usize, fallback: f64) -> f64 {
    args.get(index).and_then(|s| s.parse::<f64>().ok()).unwrap_or(fallback)
}
