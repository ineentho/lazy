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
            public_url
                .split_once("://")
                .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
                .unwrap_or(public_url)
                .to_string(),
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
