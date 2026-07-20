pub mod framework;
pub mod ports;

use framework::{FrameworkHint, find_package_script_framework, prepare_framework_args};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PreparedCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

pub fn prepare_http_command(
    mut argv: Vec<String>,
    port: u16,
    public_url: &str,
    framework: Option<&str>,
    cwd: &Path,
) -> PreparedCommand {
    let host = host_for(&argv, framework);
    let hint = framework
        .and_then(FrameworkHint::from_name)
        .or_else(|| find_package_script_framework(&argv, cwd));
    prepare_framework_args(&mut argv, port, host.as_deref(), hint);

    let mut env = vec![
        ("PORT".to_string(), port.to_string()),
        ("LAZY_URL".to_string(), public_url.to_string()),
        (
            "__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS".to_string(),
            public_hostname(public_url).to_string(),
        ),
    ];
    if let Some(host) = host {
        env.push(("HOST".to_string(), host));
    }

    PreparedCommand { argv, env }
}

fn host_for(argv: &[String], framework: Option<&str>) -> Option<String> {
    let first = argv.first().map(|s| basename(s));
    let is_expo = framework == Some("expo") || first.as_deref() == Some("expo");
    if is_expo {
        Some("localhost".to_string())
    } else {
        Some("127.0.0.1".to_string())
    }
}

fn basename(value: &str) -> String {
    std::path::Path::new(value)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(value)
        .to_string()
}

fn public_hostname(public_url: &str) -> &str {
    let without_scheme = public_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(public_url);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    authority.split(':').next().unwrap_or(authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vite_allowed_host_uses_hostname_without_port_or_path() {
        let prepared = prepare_http_command(
            vec!["npx".to_string(), "vite".to_string(), "dev".to_string()],
            4102,
            "https://node.tailnet.ts.net:18443/vite/",
            None,
            Path::new("."),
        );

        assert!(prepared.env.contains(&(
            "__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS".to_string(),
            "node.tailnet.ts.net".to_string()
        )));
    }

    #[test]
    fn detects_vite_in_a_simple_pnpm_script() {
        let cwd = test_package(r#"{"scripts":{"dev":"vite dev"}}"#);
        let prepared = prepare_http_command(
            vec!["pnpm".to_string(), "run".to_string(), "dev".to_string()],
            4103,
            "http://vite.localhost:8080",
            None,
            &cwd,
        );

        assert!(prepared.argv.ends_with(&[
            "--port".to_string(),
            "4103".to_string(),
            "--strictPort".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
        ]));
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn detects_scripts_for_supported_package_managers() {
        let cwd = test_package(r#"{"scripts":{"dev":"vite dev"}}"#);

        for argv in [
            vec!["npm", "run", "dev"],
            vec!["npm", "run-script", "dev"],
            vec!["pnpm", "run", "dev"],
            vec!["yarn", "run", "dev"],
            vec!["bun", "run", "dev"],
        ] {
            let prepared = prepare_http_command(
                argv.into_iter().map(str::to_string).collect(),
                4104,
                "http://vite.localhost:8080",
                None,
                &cwd,
            );

            assert!(prepared.argv.contains(&"--port".to_string()));
        }

        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn ignores_composed_and_wrapped_package_scripts() {
        for script in ["vite dev && echo ready", "sh -c 'vite dev'"] {
            let package = serde_json::json!({"scripts": {"dev": script}}).to_string();
            let cwd = test_package(&package);
            let prepared = prepare_http_command(
                vec!["pnpm".to_string(), "run".to_string(), "dev".to_string()],
                4105,
                "http://vite.localhost:8080",
                None,
                &cwd,
            );

            assert_eq!(prepared.argv, vec!["pnpm", "run", "dev"]);
            std::fs::remove_dir_all(cwd).unwrap();
        }
    }

    fn test_package(package_json: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let cwd =
            std::env::temp_dir().join(format!("lazy-package-script-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("package.json"), package_json).unwrap();
        cwd
    }
}
