use std::fs;
use std::process::Command;
use universe::parser::parse_subscription;
use universe::singbox::{
    build_config, build_config_with_options, RouteFlowRule, SplitTunnelMode, StatsApi,
    TunnelOptions,
};

const FEED: &str = include_str!("../assets/default-configs.txt");

#[test]
fn every_bundled_node_parses_and_generates_json() {
    let report = parse_subscription(FEED);
    assert_eq!(report.ignored, 0, "some bundled lines were not parsed");
    assert_eq!(report.nodes.len(), 93);
    for node in &report.nodes {
        build_config(node).unwrap_or_else(|error| {
            panic!(
                "{} ({}) did not generate: {error}",
                node.name,
                node.endpoint()
            )
        });
    }
}

#[test]
fn sing_box_accepts_every_generated_config_when_core_is_supplied() {
    let Ok(core) = std::env::var("SING_BOX_CHECK") else {
        return;
    };
    let feed = std::env::var("UNIVERSE_FEED")
        .ok()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_else(|| FEED.to_owned());
    let report = parse_subscription(&feed);
    assert!(
        !report.nodes.is_empty(),
        "validation feed contained no nodes"
    );
    assert_eq!(
        report.ignored, 0,
        "validation feed contained unsupported lines"
    );
    let config_path =
        std::env::temp_dir().join(format!("universe-feed-check-{}.json", std::process::id()));
    for node in &report.nodes {
        let config = build_config(node).unwrap();
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let output = Command::new(&core)
            .args(["check", "-c"])
            .arg(&config_path)
            .output()
            .unwrap_or_else(|error| panic!("could not run {core}: {error}"));
        assert!(
            output.status.success(),
            "sing-box rejected {} ({}): {}{}",
            node.name,
            node.endpoint(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let enhanced = TunnelOptions {
        strict_route: true,
        allow_lan: false,
        secure_dns: true,
        split_tunnel: true,
        split_mode: SplitTunnelMode::BypassSelected,
        split_process_names: vec!["chrome.exe".to_owned()],
        split_process_paths: vec![r"C:\Program Files\Example\example.exe".to_owned()],
        split_domains: vec!["example.com".to_owned()],
        stats_api: Some(StatsApi {
            address: "127.0.0.1:19090".to_owned(),
            secret: "secret".to_owned(),
        }),
        ..TunnelOptions::default()
    };
    let config = build_config_with_options(&report.nodes[0], &enhanced).unwrap();
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let output = Command::new(&core)
        .args(["check", "-c"])
        .arg(&config_path)
        .output()
        .unwrap_or_else(|error| panic!("could not run {core}: {error}"));
    assert!(
        output.status.success(),
        "sing-box rejected the enhanced settings config: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if report.nodes.len() >= 3 {
        let advanced = TunnelOptions {
            multihop_entry: Some(report.nodes[1].clone()),
            route_flow: vec![RouteFlowRule {
                node: report.nodes[2].clone(),
                process_names: vec!["chrome.exe".to_owned()],
                process_paths: Vec::new(),
                domains: vec!["youtube.com".to_owned()],
            }],
            ..TunnelOptions::default()
        };
        let config = build_config_with_options(&report.nodes[0], &advanced).unwrap();
        fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let output = Command::new(&core)
            .args(["check", "-c"])
            .arg(&config_path)
            .output()
            .unwrap_or_else(|error| panic!("could not run {core}: {error}"));
        assert!(
            output.status.success(),
            "sing-box rejected MultiHop/RouteFlow config: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = fs::remove_file(config_path);
}
