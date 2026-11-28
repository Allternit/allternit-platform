//! Static service table: parser for a small TOML subset, ordering, restart policy.

use std::collections::HashMap;

/// Compiled-in default, used when /etc/allternit/init.toml is absent.
pub const DEFAULT_CONFIG: &str = r#"
hostname = "allternit-os"
state_device = "/dev/vda3"
state_mount = "/state"
state_fstype = "ext4"

[[service]]
name = "authority"
path = "/usr/bin/authority"
args = ["--socket", "/run/allternit/authority.sock"]
uid = 900
gid = 900
restart = "always"

# Model: static llama-server on the baked GGUF (step 29). First version runs in the root
# network namespace on 127.0.0.1 only; the private netns + socket proxy is the ADR-0006
# follow-up (TODO). Shell waits on /health before it prompts.
[[service]]
name = "model"
path = "/usr/lib/allternit/llama-server"
args = ["-m", "/state/model/model.gguf", "--host", "127.0.0.1", "--port", "8080", "--jinja", "-c", "2048", "-t", "2", "--no-webui", "--no-warmup"]
uid = 901
gid = 901
restart = "on-failure"
after = ["authority"]

[[service]]
name = "shell"
path = "/usr/lib/allternit/shell"
args = []
uid = 902
gid = 902
env = ["HOME=/state/shell", "TERM=xterm-256color", "ALLTERNIT_AUTHORITY_SOCK=/run/allternit/authority.sock", "ALLTERNIT_MODEL_URL=http://127.0.0.1:8080"]
restart = "never"
after = ["authority", "model"]
poweroff_on_clean_exit = true
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    Never,
    OnFailure,
    Always,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub path: String,
    pub args: Vec<String>,
    pub uid: u32,
    pub gid: u32,
    pub env: Vec<(String, String)>,
    pub restart: Restart,
    pub netns: bool,
    pub after: Vec<String>,
    pub poweroff_on_clean_exit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub hostname: String,
    pub state_device: Option<String>,
    pub state_mount: String,
    pub state_fstype: String,
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Str(String),
    Int(u64),
    Bool(bool),
    List(Vec<String>),
}

type Table = HashMap<String, Val>;

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in line.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

fn parse_string(s: &str) -> Result<(String, &str), String> {
    // s starts right after the opening quote
    let mut out = String::new();
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if esc {
            out.push(match c {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Ok((out, &s[i + 1..]));
        } else {
            out.push(c);
        }
    }
    Err("unterminated string".into())
}

fn parse_value(raw: &str) -> Result<Val, String> {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix('"') {
        let (s, tail) = parse_string(rest)?;
        if !tail.trim().is_empty() {
            return Err(format!("trailing text after string: {tail}"));
        }
        return Ok(Val::Str(s));
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let mut rest = rest.trim_start();
        let mut items = Vec::new();
        loop {
            if let Some(tail) = rest.strip_prefix(']') {
                if !tail.trim().is_empty() {
                    return Err("trailing text after array".into());
                }
                return Ok(Val::List(items));
            }
            let body = rest
                .strip_prefix('"')
                .ok_or_else(|| "arrays hold strings only".to_string())?;
            let (s, tail) = parse_string(body)?;
            items.push(s);
            rest = tail.trim_start();
            if let Some(t) = rest.strip_prefix(',') {
                rest = t.trim_start();
            } else if !rest.starts_with(']') {
                return Err("expected , or ] in array".into());
            }
        }
    }
    match raw {
        "true" => return Ok(Val::Bool(true)),
        "false" => return Ok(Val::Bool(false)),
        _ => {}
    }
    raw.parse::<u64>()
        .map(Val::Int)
        .map_err(|_| format!("unsupported value: {raw}"))
}

fn get_str(t: &Table, k: &str) -> Result<Option<String>, String> {
    match t.get(k) {
        None => Ok(None),
        Some(Val::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{k} must be a string")),
    }
}

fn get_u32(t: &Table, k: &str) -> Result<Option<u32>, String> {
    match t.get(k) {
        None => Ok(None),
        Some(Val::Int(n)) => u32::try_from(*n).map(Some).map_err(|_| format!("{k} out of range")),
        Some(_) => Err(format!("{k} must be an integer")),
    }
}

fn get_bool(t: &Table, k: &str) -> Result<bool, String> {
    match t.get(k) {
        None => Ok(false),
        Some(Val::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("{k} must be true or false")),
    }
}

fn get_list(t: &Table, k: &str) -> Result<Vec<String>, String> {
    match t.get(k) {
        None => Ok(Vec::new()),
        Some(Val::List(l)) => Ok(l.clone()),
        Some(_) => Err(format!("{k} must be an array of strings")),
    }
}

fn build_service(t: &Table) -> Result<Service, String> {
    let name = get_str(t, "name")?.ok_or("service is missing name")?;
    let path = get_str(t, "path")?.ok_or_else(|| format!("service {name} is missing path"))?;
    let restart = match get_str(t, "restart")?.as_deref() {
        None | Some("never") => Restart::Never,
        Some("on-failure") => Restart::OnFailure,
        Some("always") => Restart::Always,
        Some(other) => return Err(format!("service {name}: unknown restart policy {other}")),
    };
    let mut env = Vec::new();
    for kv in get_list(t, "env")? {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("service {name}: env entry {kv} is not KEY=VALUE"))?;
        env.push((k.to_string(), v.to_string()));
    }
    let uid = get_u32(t, "uid")?.unwrap_or(0);
    Ok(Service {
        name,
        path,
        args: get_list(t, "args")?,
        uid,
        gid: get_u32(t, "gid")?.unwrap_or(uid),
        env,
        restart,
        netns: get_bool(t, "netns")?,
        after: get_list(t, "after")?,
        poweroff_on_clean_exit: get_bool(t, "poweroff_on_clean_exit")?,
    })
}

/// Parse the init.toml subset: top-level keys plus `[[service]]` tables.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut top = Table::new();
    let mut services: Vec<Table> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let err = |m: String| format!("line {}: {m}", n + 1);
        if line == "[[service]]" {
            services.push(Table::new());
            continue;
        }
        if line.starts_with('[') && !line.contains('=') {
            return Err(err(format!("unknown section {line}")));
        }
        let (k, v) = line.split_once('=').ok_or_else(|| err("expected key = value".into()))?;
        let val = parse_value(v).map_err(err)?;
        let table = services.last_mut().unwrap_or(&mut top);
        if table.insert(k.trim().to_string(), val).is_some() {
            return Err(err(format!("duplicate key {}", k.trim())));
        }
    }
    let services = services
        .iter()
        .map(build_service)
        .collect::<Result<Vec<_>, _>>()?;
    let cfg = Config {
        hostname: get_str(&top, "hostname")?.unwrap_or_else(|| "allternit-os".into()),
        state_device: get_str(&top, "state_device")?,
        state_mount: get_str(&top, "state_mount")?.unwrap_or_else(|| "/state".into()),
        state_fstype: get_str(&top, "state_fstype")?.unwrap_or_else(|| "ext4".into()),
        services,
    };
    order_services(&cfg.services)?; // validate
    Ok(cfg)
}

pub fn default_config() -> Config {
    parse(DEFAULT_CONFIG).expect("compiled-in default config is valid")
}

/// Start order: every service after the ones it names in `after`; ties keep
/// file order. Errors on duplicates, unknown dependencies and cycles.
pub fn order_services(services: &[Service]) -> Result<Vec<usize>, String> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (i, s) in services.iter().enumerate() {
        if index.insert(s.name.as_str(), i).is_some() {
            return Err(format!("duplicate service {}", s.name));
        }
    }
    for s in services {
        for dep in &s.after {
            if !index.contains_key(dep.as_str()) {
                return Err(format!("service {} runs after unknown service {dep}", s.name));
            }
        }
    }
    let mut done = vec![false; services.len()];
    let mut order = Vec::with_capacity(services.len());
    while order.len() < services.len() {
        let next = (0..services.len()).find(|&i| {
            !done[i] && services[i].after.iter().all(|d| done[index[d.as_str()]])
        });
        match next {
            Some(i) => {
                done[i] = true;
                order.push(i);
            }
            None => return Err("dependency cycle in service table".into()),
        }
    }
    Ok(order)
}

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    pub fn is_clean(self) -> bool {
        self == Exit::Code(0)
    }
}

pub fn should_restart(policy: Restart, exit: Exit) -> bool {
    match policy {
        Restart::Never => false,
        Restart::Always => true,
        Restart::OnFailure => !exit.is_clean(),
    }
}

/// What the supervisor does when a service exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Restart,
    PowerOff,
    Leave,
}

pub fn on_exit(svc: &Service, exit: Exit) -> Action {
    if svc.poweroff_on_clean_exit && exit.is_clean() {
        Action::PowerOff
    } else if should_restart(svc.restart, exit) {
        Action::Restart
    } else {
        Action::Leave
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses_and_is_ordered() {
        let c = default_config();
        let names: Vec<_> = order_services(&c.services)
            .unwrap()
            .into_iter()
            .map(|i| c.services[i].name.as_str())
            .collect();
        assert_eq!(names, ["authority", "model", "shell"]);
    }

    #[test]
    fn default_users_and_netns() {
        let c = default_config();
        let by = |n: &str| c.services.iter().find(|s| s.name == n).unwrap();
        assert_eq!((by("authority").uid, by("authority").gid), (900, 900));
        assert_eq!(by("model").uid, 901);
        // TODO(ADR-0006): flip back to assert!(netns) once the socket proxy lands.
        assert!(!by("model").netns);
        assert!(!by("authority").netns && !by("shell").netns);
        assert_eq!(by("shell").uid, 902);
        assert!(by("shell").poweroff_on_clean_exit);
    }

    #[test]
    fn parses_fields_comments_and_escapes() {
        let c = parse(
            "hostname = \"h\" # c\n[[service]]\nname = \"a\"\npath = \"/x # y\"\nargs = [\"-a\", \"b\\\"c\"]\nenv = [\"K=V=W\"]\nuid = 5\nrestart = \"on-failure\"\n",
        )
        .unwrap();
        let s = &c.services[0];
        assert_eq!(c.hostname, "h");
        assert_eq!(s.path, "/x # y");
        assert_eq!(s.args, ["-a", "b\"c"]);
        assert_eq!(s.env, [("K".to_string(), "V=W".to_string())]);
        assert_eq!((s.uid, s.gid), (5, 5));
        assert_eq!(s.restart, Restart::OnFailure);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("[[service]]\npath = \"/x\"\n").is_err()); // no name
        assert!(parse("[[service]]\nname = \"a\"\npath = \"/x\"\nrestart = \"x\"\n").is_err());
        assert!(parse("[[service]]\nname = \"a\"\npath = \"/x\"\nenv = [\"NOEQ\"]\n").is_err());
        assert!(parse("[[service]]\nname = \"a\"\nname = \"b\"\n").is_err());
        assert!(parse("junk\n").is_err());
        assert!(parse("[other]\n").is_err());
        assert!(parse("x = \"open\n").is_err());
    }

    fn svc(name: &str, after: &[&str]) -> Service {
        Service {
            name: name.into(),
            path: "/x".into(),
            args: vec![],
            uid: 1,
            gid: 1,
            env: vec![],
            restart: Restart::Never,
            netns: false,
            after: after.iter().map(|s| s.to_string()).collect(),
            poweroff_on_clean_exit: false,
        }
    }

    #[test]
    fn ordering_respects_after_and_file_order() {
        let v = vec![svc("c", &["b"]), svc("a", &[]), svc("b", &["a"]), svc("d", &[])];
        assert_eq!(order_services(&v).unwrap(), vec![1, 2, 0, 3]);
    }

    #[test]
    fn ordering_errors() {
        assert!(order_services(&[svc("a", &["b"]), svc("b", &["a"])]).is_err());
        assert!(order_services(&[svc("a", &["zzz"])]).is_err());
        assert!(order_services(&[svc("a", &[]), svc("a", &[])]).is_err());
    }

    #[test]
    fn restart_policy() {
        assert!(!should_restart(Restart::Never, Exit::Code(1)));
        assert!(should_restart(Restart::Always, Exit::Code(0)));
        assert!(should_restart(Restart::OnFailure, Exit::Signal(9)));
        assert!(!should_restart(Restart::OnFailure, Exit::Code(0)));
    }

    #[test]
    fn shell_clean_exit_powers_off() {
        let c = default_config();
        let shell = c.services.iter().find(|s| s.name == "shell").unwrap();
        assert_eq!(on_exit(shell, Exit::Code(0)), Action::PowerOff);
        assert_eq!(on_exit(shell, Exit::Code(2)), Action::Leave);
        let auth = c.services.iter().find(|s| s.name == "authority").unwrap();
        assert_eq!(on_exit(auth, Exit::Code(0)), Action::Restart);
    }
}
