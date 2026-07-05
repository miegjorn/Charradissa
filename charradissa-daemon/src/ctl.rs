//! charradissa-ctl — daily workflow CLI
//!
//! Commands:
//!   morning              Print a briefing of pending approvals.
//!   wrap                 Print end-of-day summary of resolved approvals.
//!   observe <room_id>    Poll the queue for changes in a room every 5s.

use charradissa_core::approval::{ApprovalStatus, PersistentApprovalQueue};
use std::collections::HashSet;
use std::path::PathBuf;

fn farga_url() -> String {
    std::env::var("FARGA_URL").unwrap_or_else(|_| "http://farga:7500".into())
}

fn morning() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let queue = PersistentApprovalQueue::new(farga_url());
    let pending = rt.block_on(queue.list_pending());
    println!("=== Morning Briefing ===");
    println!("Pending approvals: {}", pending.len());
    for record in &pending {
        println!(
            "  [{}] room={} [{}] {}",
            record.id, record.room_id, record.category, record.description
        );
    }
    if pending.is_empty() {
        println!("  (no pending approvals)");
    }
}

fn wrap() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let queue = PersistentApprovalQueue::new(farga_url());
    let all = rt.block_on(queue.list_all());
    let today = chrono::Utc::now().date_naive();
    let resolved_today: Vec<_> = all
        .iter()
        .filter(|r| {
            r.created_at.date_naive() == today
                && matches!(r.status, ApprovalStatus::Approved | ApprovalStatus::Rejected(_))
        })
        .collect();
    let approved = resolved_today
        .iter()
        .filter(|r| matches!(r.status, ApprovalStatus::Approved))
        .count();
    let rejected = resolved_today
        .iter()
        .filter(|r| matches!(r.status, ApprovalStatus::Rejected(_)))
        .count();
    println!("=== End-of-Day Wrap ===");
    println!(
        "Approvals resolved today: {} total ({} approved, {} rejected)",
        resolved_today.len(),
        approved,
        rejected
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("morning") => morning(),
        Some("wrap") => wrap(),
        Some("service") => {
            // Service subcommand handled in ctl.rs via install
            if args.get(2).map(|s| s.as_str()) == Some("install") {
                service_install();
            } else {
                eprintln!("Usage: charradissa-ctl service install");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("Usage: charradissa-ctl <morning|wrap|service install>");
            std::process::exit(1);
        }
    }
}

fn service_install() {
    let binary_path = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("charradissa-daemon"));
    // For service install, we want the daemon binary, not ctl
    let binary_str = binary_path
        .to_string_lossy()
        .replace("charradissa-ctl", "charradissa-daemon");

    let config_path = std::env::var("CHARRADISSA_CONFIG")
        .unwrap_or_else(|_| "charradissa.toml".into());

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let plist_dir = PathBuf::from(&home).join("Library/LaunchAgents");
        std::fs::create_dir_all(&plist_dir).ok();
        let plist_path = plist_dir.join("com.bosa.charradissa.plist");
        let content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.bosa.charradissa</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary_str}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>EnvironmentVariables</key>
    <dict>
        <key>CHARRADISSA_CONFIG</key>
        <string>{config_path}</string>
    </dict>
</dict>
</plist>
"#,
            binary_str = binary_str,
            config_path = config_path,
        );
        std::fs::write(&plist_path, content).expect("write plist");
        println!("Installed launchd plist: {}", plist_path.display());
        println!("To activate: launchctl load -w {}", plist_path.display());
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let unit_dir = PathBuf::from(&home).join(".config/systemd/user");
        std::fs::create_dir_all(&unit_dir).ok();
        let unit_path = unit_dir.join("charradissa.service");
        let content = format!(
            "[Unit]\nDescription=Charradissa Matrix Agent Daemon\nAfter=network.target\n\n[Service]\nExecStart={binary_str}\nEnvironment=CHARRADISSA_CONFIG={config_path}\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            binary_str = binary_str,
            config_path = config_path,
        );
        std::fs::write(&unit_path, content).expect("write unit file");
        println!("Installed systemd unit: {}", unit_path.display());
        println!("To activate:");
        println!("  systemctl --user daemon-reload");
        println!("  systemctl --user enable --now charradissa.service");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("Service install not supported on this platform.");
        std::process::exit(1);
    }
}
