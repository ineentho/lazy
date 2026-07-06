use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameworkHint {
    name: &'static str,
    strict_port: bool,
}

impl FrameworkHint {
    pub fn from_name(name: &str) -> Option<Self> {
        framework(name)
    }
}

const PACKAGE_RUNNERS: &[(&str, &[&str])] = &[
    ("npx", &[]),
    ("bunx", &[]),
    ("pnpx", &[]),
    ("yarn", &["dlx", "exec"]),
    ("pnpm", &["dlx", "exec"]),
];

pub fn prepare_framework_args(
    argv: &mut Vec<String>,
    port: u16,
    host: Option<&str>,
    hint: Option<FrameworkHint>,
) {
    let Some(framework) = hint.or_else(|| find_framework(argv)) else {
        return;
    };

    if !has_flag(argv, "--port") {
        argv.push("--port".to_string());
        argv.push(port.to_string());
        if framework.strict_port && !has_flag(argv, "--strictPort") {
            argv.push("--strictPort".to_string());
        }
    }

    if !has_flag(argv, "--host") {
        if let Some(host) = host {
            let host = if framework.name == "expo" {
                "localhost"
            } else {
                host
            };
            argv.push("--host".to_string());
            argv.push(host.to_string());
        }
    }
}

fn find_framework(argv: &[String]) -> Option<FrameworkHint> {
    let first = argv.first().map(|s| basename(s))?;
    if let Some(framework) = framework(&first) {
        return Some(framework);
    }

    let (_, subcommands) = PACKAGE_RUNNERS
        .iter()
        .find(|(runner, _)| *runner == first.as_str())?;

    let mut i = 1;
    if !subcommands.is_empty() {
        while i < argv.len() && argv[i].starts_with('-') {
            i += 1;
        }
        if i >= argv.len() {
            return None;
        }
        if !subcommands.contains(&argv[i].as_str()) {
            return framework(&basename(&argv[i]));
        }
        i += 1;
    }

    while i < argv.len() && argv[i].starts_with('-') {
        i += 1;
    }
    argv.get(i).and_then(|arg| framework(&basename(arg)))
}

fn framework(name: &str) -> Option<FrameworkHint> {
    let strict_port = match name {
        "vite" | "vp" | "react-router" => true,
        "rsbuild" | "astro" | "ng" | "react-native" | "expo" => false,
        _ => return None,
    };
    Some(FrameworkHint {
        name: match name {
            "vite" => "vite",
            "vp" => "vp",
            "react-router" => "react-router",
            "rsbuild" => "rsbuild",
            "astro" => "astro",
            "ng" => "ng",
            "react-native" => "react-native",
            "expo" => "expo",
            _ => unreachable!(),
        },
        strict_port,
    })
}

fn has_flag(argv: &[String], flag: &str) -> bool {
    argv.iter().any(|arg| arg == flag)
}

fn basename(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(value)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_vite_port_host_and_strict_port() {
        let mut argv = vec!["vite".to_string(), "dev".to_string()];

        prepare_framework_args(&mut argv, 4100, Some("127.0.0.1"), None);

        assert_eq!(
            argv,
            vec![
                "vite",
                "dev",
                "--port",
                "4100",
                "--strictPort",
                "--host",
                "127.0.0.1"
            ]
        );
    }

    #[test]
    fn looks_through_package_runner_flags() {
        let mut argv = vec!["pnpm".to_string(), "dlx".to_string(), "vite".to_string()];

        prepare_framework_args(&mut argv, 4200, Some("127.0.0.1"), None);

        assert!(argv.ends_with(&[
            "--port".to_string(),
            "4200".to_string(),
            "--strictPort".to_string(),
            "--host".to_string(),
            "127.0.0.1".to_string(),
        ]));
    }

    #[test]
    fn explicit_framework_hint_covers_hidden_scripts() {
        let mut argv = vec!["pnpm".to_string(), "dev".to_string()];

        prepare_framework_args(
            &mut argv,
            4300,
            Some("127.0.0.1"),
            FrameworkHint::from_name("vite"),
        );

        assert!(argv.contains(&"--port".to_string()));
        assert!(argv.contains(&"4300".to_string()));
        assert!(argv.contains(&"--strictPort".to_string()));
    }

    #[test]
    fn expo_uses_localhost_host_value() {
        let mut argv = vec!["expo".to_string(), "start".to_string()];

        prepare_framework_args(&mut argv, 4400, Some("127.0.0.1"), None);

        assert_eq!(
            argv,
            vec!["expo", "start", "--port", "4400", "--host", "localhost"]
        );
    }

    #[test]
    fn respects_existing_port_and_host_flags() {
        let mut argv = vec![
            "vite".to_string(),
            "--port".to_string(),
            "3000".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
        ];

        prepare_framework_args(&mut argv, 4500, Some("127.0.0.1"), None);

        assert_eq!(argv, vec!["vite", "--port", "3000", "--host", "0.0.0.0"]);
    }
}
