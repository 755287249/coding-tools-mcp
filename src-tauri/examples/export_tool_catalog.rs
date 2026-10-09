fn main() {
    let profiles = [
        "advanced",
        "read-only",
        "compat-readonly-all",
        "guarded-core",
        "trusted-core",
    ];
    let catalogs = profiles
        .into_iter()
        .map(|profile| {
            (
                profile.to_string(),
                serde_json::json!({
                    "tools": coding_tools_mcp_desktop_lib::tools::registry::list_tools_for_profile(profile),
                    "toolset_revision": coding_tools_mcp_desktop_lib::tools::registry::toolset_revision(profile)
                }),
            )
        })
        .collect::<serde_json::Map<String, serde_json::Value>>();
    let output = serde_json::json!({
        "profiles": catalogs,
        "local_chat_skill": coding_tools_mcp_desktop_lib::tools::chat::local_chat_skill(),
        "behavioral_parity": coding_tools_mcp_desktop_lib::export_behavioral_parity_fixtures()
    });
    println!(
        "{}",
        serde_json::to_string(&output).expect("tool catalog is serializable")
    );
}
