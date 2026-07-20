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

    let inject_port = !has_flag(argv, "--port");
    let inject_host = !has_flag(argv, "--host") && host.is_some();
    if (inject_port || inject_host) && needs_argument_separator(argv) {
        argv.push("--".to_string());
    }

    if inject_port {
        argv.push("--port".to_string());
        argv.push(port.to_string());
        if framework.strict_port && !has_flag(argv, "--strictPort") {
            argv.push("--strictPort".to_string());
        }
    }

    if inject_host && let Some(host) = host {
        let host = if framework.name == "expo" {
            "localhost"
        } else {
            host
        };
        argv.push("--host".to_string());
        argv.push(host.to_string());
    }
}

pub fn find_package_script_framework(argv: &[String], cwd: &Path) -> Option<FrameworkHint> {
    let script_name = package_script_name(argv)?;
    let package_json = std::fs::read_to_string(cwd.join("package.json")).ok()?;
    let package: serde_json::Value = serde_json::from_str(&package_json).ok()?;
    let script = package.get("scripts")?.get(script_name)?.as_str()?;

    // Shell composition changes where injected arguments land, so only resolve a
    // script whose first command is also its only command.
    if script
        .chars()
        .any(|c| matches!(c, '&' | '|' | ';' | '<' | '>' | '`' | '#' | '\n' | '\r'))
        || script.contains("$(")
    {
        return None;
    }

    let words = shell_words::split(script).ok()?;
    words.first().and_then(|word| framework(&basename(word)))
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

fn package_script_name(argv: &[String]) -> Option<&str> {
    let runner = argv.first().map(|arg| basename(arg))?;
    let subcommand = argv.get(1)?.as_str();
    let is_run = match runner.as_str() {
        "npm" => matches!(subcommand, "run" | "run-script"),
        "pnpm" | "yarn" | "bun" => subcommand == "run",
        _ => false,
    };

    if is_run {
        argv.get(2).map(String::as_str)
    } else {
        None
    }
}

fn needs_argument_separator(argv: &[String]) -> bool {
    argv.first().map(|arg| basename(arg)).as_deref() == Some("npm")
        && matches!(argv.get(1).map(String::as_str), Some("run" | "run-script"))
        && !argv.iter().skip(3).any(|arg| arg == "--")
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
    fn npm_script_arguments_use_forwarding_separator() {
        let mut argv = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];

        prepare_framework_args(
            &mut argv,
            4350,
            Some("127.0.0.1"),
            FrameworkHint::from_name("vite"),
        );

        assert_eq!(
            argv,
            vec![
                "npm",
                "run",
                "dev",
                "--",
                "--port",
                "4350",
                "--strictPort",
                "--host",
                "127.0.0.1"
            ]
        );
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
