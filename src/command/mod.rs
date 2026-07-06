pub mod framework;
pub mod ports;

use framework::{FrameworkHint, prepare_framework_args};

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
) -> PreparedCommand {
    let host = host_for(&argv, framework);
    let hint = framework.and_then(FrameworkHint::from_name);
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
        );

        assert!(prepared.env.contains(&(
            "__VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS".to_string(),
            "node.tailnet.ts.net".to_string()
        )));
    }
}
